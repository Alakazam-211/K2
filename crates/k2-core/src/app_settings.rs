//! Daemon-owned global application settings (`~/.k2so/settings.json`).
//!
//! Phase 2 Unit 7a — moves the `AppSettings` schema + on-disk
//! tmp-and-rename writer from `src-tauri/src/commands/settings.rs` into
//! k2so-core so the daemon can serve `/cli/settings/{get,update,reset}`
//! without Tauri being present.
//!
//! # Two-writers race (F3) — resolved here
//!
//! Pre-Unit-7a, both Tauri (`settings_update`) and the daemon
//! (`companion_host::persist_companion_password_fields`) wrote to the
//! same `~/.k2so/settings.json`. tmp+rename meant readers never saw a
//! torn file, but a Tauri-side rotate of `companion.username` was a
//! no-op on the daemon's in-memory companion runtime (Tauri's `STATE`
//! is empty post-Unit-1). The legacy live tokens stayed valid until
//! their TTL expired.
//!
//! After Unit 7a, Tauri's `settings_update` proxies to
//! `POST /cli/settings/update`. The daemon takes the global
//! [`SETTINGS_LOCK`] before load+merge+save, then — after the file is
//! durable — compares old-vs-new companion-affecting fields and calls
//! `k2_core::companion::invalidate_all_sessions` server-side, where
//! the live `STATE` actually lives. The race window collapses to zero:
//! a single critical section owns the merge, and the post-write
//! invalidation is in-process with the sessions it's killing.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;


// ── Settings types (moved verbatim from src-tauri/src/commands/settings.rs) ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    #[serde(default = "default_scrollback")]
    pub scrollback: u32,
    #[serde(default = "default_true")]
    pub natural_text_editing: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        default_terminal()
    }
}

fn default_font_family() -> String {
    "MesloLGM Nerd Font".to_string()
}
fn default_font_size() -> u32 {
    13
}
fn default_cursor_style() -> String {
    "bar".to_string()
}
fn default_scrollback() -> u32 {
    5000
}

fn default_agent() -> String {
    "claude".to_string()
}

/// K2 Mail (prd-email-server-v1 §8.4 / D4) — GLOBAL default agent-send
/// gating mode. **MUST stay "off"**: outbound email is opt-in, per
/// workspace, fail-closed.
fn default_mail_agent_send() -> String {
    "off".to_string()
}

/// K2 Mail (prd-email-server-v1 §7.2 / D6) — GLOBAL default address
/// cap per agent. 0 = unlimited.
fn default_mail_address_cap() -> u32 {
    5
}

/// P1.C — default Active-Bar tenure window: 24 hours.
fn default_active_window_hours() -> u32 {
    24
}

fn default_session_archive_days() -> u32 {
    14
}

fn default_left_panel_tab() -> String {
    "files".to_string()
}
fn default_right_panel_tab() -> String {
    "history".to_string()
}
fn default_left_panel_tabs() -> Vec<String> {
    vec!["files".to_string(), "agents".to_string()]
}
fn default_right_panel_tabs() -> Vec<String> {
    vec![
        "history".to_string(),
        "changes".to_string(),
        "reviews".to_string(),
    ]
}

fn default_terminal() -> TerminalSettings {
    TerminalSettings {
        font_family: "MesloLGM Nerd Font".to_string(),
        font_size: 13,
        cursor_style: "bar".to_string(),
        scrollback: 5000,
        natural_text_editing: true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimerSettings {
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default = "default_true")]
    pub countdown_enabled: bool,
    #[serde(default = "default_countdown_theme")]
    pub countdown_theme: String,
    #[serde(default)]
    pub skip_memo: bool,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub custom_themes: Vec<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

fn default_countdown_theme() -> String {
    "rocket".to_string()
}

impl Default for TimerSettings {
    fn default() -> Self {
        Self {
            visible: true,
            countdown_enabled: true,
            countdown_theme: "rocket".to_string(),
            skip_memo: false,
            timezone: String::new(),
            custom_themes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_terminal")]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub keybindings: HashMap<String, String>,
    #[serde(default, alias = "projects")]
    pub project_settings: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub focus_groups_enabled: bool,
    #[serde(default)]
    pub active_focus_group_id: Option<String>,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub left_panel_open: bool,
    #[serde(default)]
    pub right_panel_open: bool,
    #[serde(default = "default_left_panel_tab")]
    pub left_panel_active_tab: String,
    #[serde(default = "default_right_panel_tab")]
    pub right_panel_active_tab: String,
    #[serde(default = "default_left_panel_tabs")]
    pub left_panel_tabs: Vec<String>,
    #[serde(default = "default_right_panel_tabs")]
    pub right_panel_tabs: Vec<String>,
    /// Session-archive sweep threshold in days (0 = disabled). Sessions
    /// older than this are COPIED into `.k2/session-archive/` daily to
    /// outlive provider transcript reaping (~30d). See session_archive.rs.
    #[serde(default = "default_session_archive_days")]
    pub session_archive_days: u32,
    /// Deprecated: workspace layouts now stored in SQLite workspace_layouts table.
    /// Kept for deserialization compat with old settings.json files; skipped on write.
    #[serde(default, skip_serializing)]
    pub workspace_layouts: HashMap<String, serde_json::Value>,
    #[serde(default = "default_agent")]
    pub default_agent: String,
    #[serde(default)]
    pub ai_assistant_enabled: bool,
    #[serde(default)]
    pub timer: TimerSettings,
    #[serde(default)]
    pub agentic_systems_enabled: bool,
    #[serde(default = "default_true")]
    pub keep_daemon_on_quit: bool,
    #[serde(default)]
    pub claude_auth_auto_refresh: bool,
    #[serde(default)]
    pub last_active_project_id: Option<String>,
    #[serde(default)]
    pub last_active_workspace_id: Option<String>,
    /// P1.C — how many hours a workspace stays in the Active Bar after the
    /// user last interacted with it (Active-Bar rule 2). Persisted here so
    /// both `ActiveBar` and a future reaper read one source of truth.
    /// A typed field is required: `AppSettings` round-trips through
    /// `serde_json::from_value` on every `load`/`update`, which silently
    /// drops keys with no matching field — an untyped key would never
    /// persist. Defaults to 24 for old settings.json snapshots.
    #[serde(default = "default_active_window_hours")]
    pub active_window_hours: u32,
    /// User-set "your display name" — the name K2 agents recognize the
    /// OWNER by. Resolved server-side as the `from` attribution when the
    /// owner sends via the composer (`/cli/terminal/send-message`); the
    /// route NEVER reads `from` from the request body (D3). `None`/blank
    /// falls back to the literal `"owner"`. A typed `Option<String>` is
    /// required: `AppSettings` round-trips through `serde_json::from_value`
    /// on every `load`/`update`, which silently drops keys with no matching
    /// field — an untyped key would never persist.
    #[serde(default)]
    pub owner_display_name: Option<String>,
    /// Composer 1c (D4 / red-team H1) — per-host opt-in that lets a
    /// CONNECT-USER (role >= Member) instruct agents via the composer route
    /// `POST /cli/terminal/send-message`. The OWNER token is ALWAYS allowed
    /// regardless of this flag; this gates ONLY remote multi-user
    /// instruction. **Defaults OFF**: the composer route instructs an agent
    /// running `--dangerously-skip-permissions` (= full RCE), so a daemon
    /// must never accept connect-user instructions until the owner
    /// explicitly opts this host in. The gate is server-enforced
    /// (drain-then-403 in the dispatcher); the renderer-hide is
    /// defense-in-depth only. A typed `bool` is required: `AppSettings`
    /// round-trips through `serde_json::from_value` on every `load`/`update`,
    /// which silently drops keys with no matching field — an untyped key
    /// would never persist.
    ///
    /// PRD note: the PRD calls for a *per-workspace* opt-in; this app-level
    /// flag is the safe-default 1c implementation, with the per-workspace
    /// refinement tracked as follow-up. Default MUST stay OFF either way.
    #[serde(default)]
    pub allow_remote_instruct: bool,
    /// DNS K1 — per-host opt-in that lets agents manage DNS records
    /// (create/update/delete at the registrar / zone). **Defaults OFF**
    /// (deny-by-default): DNS mutation is a high-impact capability, so a
    /// daemon must never grant it until the owner explicitly opts this
    /// host in. A typed `bool` is required: `AppSettings` round-trips
    /// through `serde_json::from_value` on every `load`/`update`, which
    /// silently drops keys with no matching field — an untyped key would
    /// never persist.
    ///
    /// Wire name `dnsManageEnabled` (serde camelCase). Per-workspace
    /// refinement lives on `projects.dns_manage_enabled` (migration 0079);
    /// effective gate is `workspace::settings::dns_manage_allowed_for_path`
    /// (app master OR per-workspace, fail-closed).
    #[serde(default)]
    pub dns_manage_enabled: bool,
    /// C1 (0.40.45) — per-host opt-in that lets agents add/remove workspace
    /// connections (`k2 connections add|remove`, relations create/delete).
    /// **Defaults OFF** (deny-by-default): wiring workspaces is a high-impact
    /// trust decision, so a daemon must never grant it until the owner
    /// explicitly opts this host in. A typed `bool` is required: `AppSettings`
    /// round-trips through `serde_json::from_value` on every `load`/`update`,
    /// which silently drops keys with no matching field.
    ///
    /// Wire name `agentsCanCreateConnections` (serde camelCase). Per-workspace
    /// refinement lives on `projects.agents_can_create_connections`
    /// (migration 0085); effective gate is
    /// `workspace::settings::agents_can_create_connections_for_path`
    /// (app master OR per-workspace, fail-closed). Owner / Owner-role
    /// connect-user always bypass the gate.
    #[serde(default)]
    pub agents_can_create_connections: bool,
    /// Cross-server federation master switch (K2 Connect → Enable federation).
    /// Persisted app-level mirror of the `K2_FEDERATION` env var so the owner
    /// can turn federation on/off per-server from the UI without editing a
    /// launchd plist + restarting. `federation::enabled()` is true when EITHER
    /// the env var OR this flag is set. Default MUST stay OFF (dark by default).
    #[serde(default)]
    pub federation_enabled: bool,
    /// 0.40.43 (1c) — public `/v1` API master switch (K2 Connect → Enable
    /// public API). Persisted app-level mirror of the `K2_API` env flag so
    /// the owner can flip the external `/v1` surface per-server from Settings
    /// without editing a launchd plist / systemd drop-in + restarting the
    /// daemon. The daemon's `misc_routes::api_enabled()` gate is true when
    /// EITHER the env flag OR this setting is on (env stays a valid force-on
    /// override for headless boxes — nsi's systemd drop-in keeps working).
    /// The gate consults the [`api_enabled_setting`] runtime mirror PER
    /// REQUEST, so the Settings toggle takes effect live — no restart, no
    /// confirm+reboot dialog. Default MUST stay OFF (surface dark by
    /// default). A typed `bool` is required: `AppSettings` round-trips
    /// through `serde_json::from_value` on every `load`/`update`, which
    /// silently drops keys with no matching field.
    #[serde(default)]
    pub api_enabled: bool,
    /// Remote Session Layer 0 — master switch for remote shell/PTY sessions
    /// driven over Connect / API. **Defaults OFF** (fail-closed): independent
    /// of grants; when this is off, every remote-session drive attempt is
    /// denied with `REMOTE_SESSIONS_DISABLED` and audited. A typed `bool` is
    /// required: `AppSettings` round-trips through `serde_json::from_value`
    /// on every `load`/`update`, which silently drops keys with no matching
    /// field — an untyped key would never persist.
    ///
    /// Wire name `remoteSessionsEnabled` (serde camelCase).
    #[serde(default)]
    pub remote_sessions_enabled: bool,
    /// Hosted web client Layer 0 — master switch for browser access via
    /// the edge-served SPA (`<sub>.app.k2.dev` / local Caddy). **Defaults
    /// ON** (product default — PRD §6.7 / §9.4): the web client is a
    /// headline feature and the tunnel already exposes token-gated
    /// `/cli/*`; this toggle lets an owner shut the browser door without
    /// disabling CLI/desktop query-token clients. When OFF, the daemon
    /// rejects data-plane requests that look like the web client
    /// (`X-K2-Client: web`, `k2_session` cookie, or Host contains
    /// `.app.k2.dev`) with teaching code `WEB_CLIENT_DISABLED` — distinct
    /// from `REMOTE_SESSIONS_DISABLED`. Unauthenticated `/boot-status`
    /// stays up and advertises `webClient.enabled` so the loader does not
    /// look dead. A typed `bool` is required: `AppSettings` round-trips
    /// through `serde_json::from_value` on every `load`/`update`, which
    /// silently drops keys with no matching field — an untyped key would
    /// never persist.
    ///
    /// Wire name `webClientEnabled` (serde camelCase).
    #[serde(default = "default_true")]
    pub web_client_enabled: bool,
    /// GH#8 — "Use local LLM to detect HITL" opt-in (Settings → General).
    /// Gates whether the `talk` CLI tool's `/cli/terminal/classify`
    /// detection step is allowed to run the bundled 1.5B model.
    /// **Defaults OFF**: with the checkbox off, classify does the regex
    /// fast-path ONLY (free, no inference) and returns `unavailable` when
    /// no marker fires — so `talk` falls back to today's regex/manual
    /// behavior; with it on, classify additionally runs the qwen model to
    /// catch unmarked HITLs the regex misses (costs battery/CPU). A typed
    /// `bool` is required: `AppSettings` round-trips through
    /// `serde_json::from_value` on every `load`/`update`, which silently
    /// drops keys with no matching field — an untyped key would never
    /// persist.
    #[serde(default)]
    pub use_llm_hitl_detection: bool,
    /// Companion C4 (prd-companion-v2 §4) — the mobile-push relay
    /// gateway base URL (e.g. `https://push.k2.dev`). Env override:
    /// `K2_PUSH_GATEWAY_URL`. **`None`/blank = push is DORMANT**: the
    /// `push::K2Cloud` adapter is never constructed and dispatch is a
    /// startup no-op — the shipped-but-inactive state until the relay
    /// gateway is deployed. Typed `Option<String>` required (the
    /// `load`/`update` round-trip drops untyped keys, see above).
    #[serde(default)]
    pub push_gateway_url: Option<String>,
    /// Companion C4 — the shared gateway token `K2Cloud` presents to
    /// `POST <gateway>/push/dispatch`. Env override:
    /// `K2_PUSH_GATEWAY_TOKEN`. Both URL and token must be set for
    /// push to leave dormancy.
    #[serde(default)]
    pub push_gateway_token: Option<String>,
    /// K2 Mail (prd-email-server-v1 §12 / D4) — the GLOBAL default
    /// agent-send gating mode: `off` (default) | `approval` | `on`.
    /// Per-workspace `projects.mail_agent_send` (migration 0075)
    /// overrides it; the effective resolver is
    /// `workspace::settings::mail_agent_send_for_path`, which
    /// fail-closes to "off" on any unknown stored value. A typed field
    /// is required: `AppSettings` round-trips through
    /// `serde_json::from_value` on every `load`/`update`, which
    /// silently drops keys with no matching field — an untyped key
    /// would never persist.
    #[serde(default = "default_mail_agent_send")]
    pub mail_agent_send: String,
    /// K2 Mail (prd-email-server-v1 §12 / D6) — the GLOBAL default
    /// address cap per agent (0 = unlimited). Per-workspace
    /// `projects.mail_address_cap` overrides it; effective resolver:
    /// `workspace::settings::mail_address_cap_for_path`.
    #[serde(default = "default_mail_address_cap")]
    pub mail_address_cap: u32,
    /// K2 Mail S2 (prd-email-server-v1 §11.1.2) — the GLOBAL default
    /// mail domain agents mint on when they don't name one. Stored in
    /// NORMALIZED form (lowercase punycode A-label; the write path
    /// runs `mail_domain::normalize_mail_domain`). Empty (the default)
    /// = "the first Verified domain" — resolved at read time by
    /// `k2-daemon mail::domains::default_domain`, which also falls
    /// back to first-verified when the configured value is no longer
    /// a Verified domain. A typed field is required: `AppSettings`
    /// round-trips through `serde_json::from_value` on every
    /// `load`/`update`, which silently drops unknown keys.
    #[serde(default)]
    pub mail_default_domain: String,
    /// K2 Mail BYO OAuth (S1 — bring-your-own OAuth client) — the
    /// owner's OWN Google/Gmail OAuth client id, used INSTEAD of the
    /// baked default at link/refresh time when non-empty. A client id
    /// is PUBLIC (not a secret); the paired Gmail client SECRET lives
    /// in the daemon vault (`mail::oauth_config`), NEVER here and never
    /// in any DB column. Empty (the default) = use the baked default.
    /// A typed field is required: `AppSettings` round-trips through
    /// `serde_json::from_value` on every `load`/`update`, which silently
    /// drops keys with no matching field — an untyped key would never
    /// persist.
    #[serde(default)]
    pub mail_oauth_gmail_client_id: String,
    /// K2 Mail BYO OAuth (S1) — the owner's OWN Microsoft/Azure PUBLIC
    /// client id, used instead of the baked default when non-empty.
    /// Microsoft device-code is a TRUE public client: there is NO client
    /// secret, ever (the set route rejects one). Empty = baked default.
    #[serde(default)]
    pub mail_oauth_microsoft_client_id: String,
    #[serde(default)]
    pub editor: EditorSettings,
    /// Style System P3 — the user's Style selection (style / palette /
    /// scheme / gaps), daemon-canonical with a renderer localStorage
    /// mirror for the pre-connect first paint. A typed struct is
    /// required: `AppSettings` round-trips through
    /// `serde_json::from_value` on every `load`/`update`, which silently
    /// drops keys with no matching field — an untyped key would never
    /// persist.
    #[serde(default)]
    pub style: StyleSettings,
    #[serde(default)]
    pub companion: CompanionSettings,
    /// Heartbeat wake scheduler — controls whether/how often launchd
    /// wakes the laptop to fire agent heartbeats.
    #[serde(default)]
    pub wake_scheduler: WakeSchedulerSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WakeSchedulerSettings {
    #[serde(default = "default_wake_mode")]
    pub mode: String,
    #[serde(default = "default_wake_interval")]
    pub interval_minutes: u32,
    /// `WakeSystem: true` in the plist — lets launchd wake the laptop
    /// from sleep to fire the heartbeat.
    #[serde(default)]
    pub wake_system: bool,
}

fn default_wake_mode() -> String {
    "on_demand".to_string()
}
fn default_wake_interval() -> u32 {
    1
}

impl Default for WakeSchedulerSettings {
    fn default() -> Self {
        Self {
            mode: default_wake_mode(),
            interval_minutes: default_wake_interval(),
            wake_system: false,
        }
    }
}

/// Mobile Companion API settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub username: String,
    /// Legacy on-disk copy of the argon2 password hash. Post-0.32.12 the
    /// canonical hash lives in the macOS Keychain; this field is cleared
    /// after migration.
    #[serde(default)]
    pub password_hash: String,
    /// True once a companion password has been configured.
    #[serde(default)]
    pub password_set: bool,
    #[serde(default)]
    pub ngrok_auth_token: String,
    #[serde(default)]
    pub ngrok_domain: String,
    /// Allowlist of browser origins permitted to use the companion API.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// When false (default), the companion API refuses
    /// `/companion/terminal/spawn` and `/companion/terminal/spawn-background`.
    #[serde(default)]
    pub allow_remote_spawn: bool,
}

/// Style System P3 (prd-style-system-v1 §5/§6) — persisted Style
/// selection. `scheme` is the user's MODE ('dark' | 'light' | 'auto');
/// 'auto' resolution against the OS appearance happens renderer-side.
/// `gaps` is '' for the style's base/compact density or one of the
/// style's declared gap presets (e.g. "regular" / "spacious").
/// The daemon stores these opaquely — validation against the style
/// registry lives in the renderer, which owns `styles.generated.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StyleSettings {
    #[serde(default = "default_style_id")]
    pub id: String,
    #[serde(default = "default_style_palette")]
    pub palette: String,
    #[serde(default = "default_style_scheme")]
    pub scheme: String,
    #[serde(default)]
    pub gaps: String,
}

fn default_style_id() -> String {
    "square".to_string()
}
fn default_style_palette() -> String {
    "charcoal".to_string()
}
fn default_style_scheme() -> String {
    "dark".to_string()
}

impl Default for StyleSettings {
    fn default() -> Self {
        Self {
            id: default_style_id(),
            palette: default_style_palette(),
            scheme: default_style_scheme(),
            gaps: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditorSettings {
    #[serde(default = "default_tab_size")]
    pub tab_size: u32,
    #[serde(default)]
    pub word_wrap: bool,
    #[serde(default)]
    pub show_whitespace: bool,
    #[serde(default = "default_editor_font_size")]
    pub font_size: u32,
    #[serde(default = "default_true")]
    pub indent_guides: bool,
    #[serde(default = "default_true")]
    pub fold_gutter: bool,
    #[serde(default = "default_true")]
    pub autocomplete: bool,
    #[serde(default = "default_true")]
    pub bracket_matching: bool,
    #[serde(default = "default_true")]
    pub line_numbers: bool,
    #[serde(default = "default_true")]
    pub highlight_active_line: bool,
    #[serde(default)]
    pub sticky_scroll: bool,
    #[serde(default)]
    pub minimap: bool,
    #[serde(default = "default_editor_theme")]
    pub theme: String,
    #[serde(default = "default_editor_font_family")]
    pub font_family: String,
    #[serde(default)]
    pub font_ligatures: bool,
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    #[serde(default = "default_true")]
    pub cursor_blink: bool,
    #[serde(default)]
    pub scroll_past_end: bool,
    #[serde(default = "default_true")]
    pub scrollbar_annotations: bool,
    #[serde(default = "default_diff_style")]
    pub diff_style: String,
    #[serde(default)]
    pub format_on_save: bool,
    #[serde(default)]
    pub vim_mode: bool,
}

fn default_tab_size() -> u32 {
    2
}
fn default_editor_font_size() -> u32 {
    12
}
fn default_diff_style() -> String {
    "gutter".to_string()
}
fn default_editor_theme() -> String {
    "k2so-dark".to_string()
}
fn default_editor_font_family() -> String {
    "MesloLGM Nerd Font".to_string()
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 2,
            word_wrap: false,
            show_whitespace: false,
            font_size: 12,
            indent_guides: true,
            fold_gutter: true,
            autocomplete: true,
            bracket_matching: true,
            line_numbers: true,
            highlight_active_line: true,
            sticky_scroll: false,
            minimap: false,
            theme: "k2so-dark".to_string(),
            font_family: "MesloLGM Nerd Font".to_string(),
            font_ligatures: false,
            cursor_style: "bar".to_string(),
            cursor_blink: true,
            scroll_past_end: false,
            scrollbar_annotations: true,
            diff_style: "gutter".to_string(),
            format_on_save: false,
            vim_mode: false,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            terminal: default_terminal(),
            session_archive_days: default_session_archive_days(),
            keybindings: HashMap::new(),
            project_settings: HashMap::new(),
            focus_groups_enabled: false,
            active_focus_group_id: None,
            sidebar_collapsed: false,
            left_panel_open: false,
            right_panel_open: false,
            left_panel_active_tab: default_left_panel_tab(),
            right_panel_active_tab: default_right_panel_tab(),
            left_panel_tabs: default_left_panel_tabs(),
            right_panel_tabs: default_right_panel_tabs(),
            workspace_layouts: HashMap::new(),
            default_agent: "claude".to_string(),
            ai_assistant_enabled: false,
            timer: TimerSettings::default(),
            agentic_systems_enabled: true,
            keep_daemon_on_quit: true,
            claude_auth_auto_refresh: false,
            last_active_project_id: None,
            last_active_workspace_id: None,
            active_window_hours: default_active_window_hours(),
            owner_display_name: None,
            allow_remote_instruct: false,
            dns_manage_enabled: false,
            agents_can_create_connections: false,
            federation_enabled: false,
            api_enabled: false,
            remote_sessions_enabled: false,
            web_client_enabled: true,
            use_llm_hitl_detection: false,
            push_gateway_url: None,
            push_gateway_token: None,
            mail_agent_send: default_mail_agent_send(),
            mail_address_cap: default_mail_address_cap(),
            mail_default_domain: String::new(),
            mail_oauth_gmail_client_id: String::new(),
            mail_oauth_microsoft_client_id: String::new(),
            editor: EditorSettings::default(),
            style: StyleSettings::default(),
            companion: CompanionSettings::default(),
            wake_scheduler: WakeSchedulerSettings::default(),
        }
    }
}

// ── 0.40.43 (1c) — public /v1 API switch, runtime mirror ─────────────────
//
// Same shape as `federation::SETTING_ENABLED`: the daemon syncs the
// persisted `AppSettings.api_enabled` value into this process at boot and
// after every `/cli/settings/{update,reset}`, so the per-request gate
// (`misc_routes::api_enabled()`) stays a pure, cheap, thread-safe check —
// no per-request disk read — yet the Settings toggle flips the `/v1`
// surface live, with NO daemon restart and no confirm+reboot dialog.

static API_SETTING_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Sync the persisted `api_enabled` setting into this process. The daemon
/// calls this at boot and after a `/cli/settings/update` or `/reset` that
/// (re)establishes the flag's value.
pub fn set_api_enabled(on: bool) {
    API_SETTING_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The runtime mirror of the persisted `api_enabled` setting — one leg of
/// the daemon's `misc_routes::api_enabled()` OR (the other legs are the
/// `K2_API` env flag and the legacy `K2_SANDBOX_API` implication). Reads
/// the atomic only; never touches disk.
pub fn api_enabled_setting() -> bool {
    API_SETTING_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

// ── File I/O ─────────────────────────────────────────────────────────────

fn settings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| {
            log_debug!("[app_settings] WARNING: no home dir, using current dir");
            PathBuf::from(".")
        })
        .join(".k2")
}

fn settings_file() -> PathBuf {
    settings_dir().join("settings.json")
}

fn ensure_dir() {
    let dir = settings_dir();
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
}

/// Recursive deep-merge of `overlay` into `base`. Matches the behavior
/// of the legacy Tauri `deep_merge` so old settings.json files with
/// partial overlays continue to round-trip identically.
fn deep_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    if let (Some(base_map), Some(overlay_map)) = (base.as_object_mut(), overlay.as_object()) {
        for (key, val) in overlay_map {
            if val.is_object() && base_map.get(key).map_or(false, |v| v.is_object()) {
                let mut base_val = base_map.get(key).unwrap().clone();
                deep_merge(&mut base_val, val);
                base_map.insert(key.clone(), base_val);
            } else {
                base_map.insert(key.clone(), val.clone());
            }
        }
    }
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(file, fs::Permissions::from_mode(0o600)) {
        log_debug!(
            "[app_settings] WARN: chmod 0o600 on {}: {}",
            file.display(),
            e
        );
    }
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

#[cfg(unix)]
fn restrict_mode_if_wider(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(file) {
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            if let Err(e) = fs::set_permissions(file, fs::Permissions::from_mode(0o600)) {
                log_debug!(
                    "[app_settings] WARN: tighten mode on {}: {}",
                    file.display(),
                    e
                );
            }
        }
    }
}

#[cfg(not(unix))]
fn restrict_mode_if_wider(_file: &Path) {}

/// Process-wide lock around `load + merge + save` critical sections.
/// The on-disk file uses tmp+rename so readers never see a torn write,
/// but the lock makes the merge atomic across concurrent
/// `update(partial)` calls inside one process. Sufficient because the
/// daemon is the sole writer now (the Tauri-side proxy hits the daemon
/// over HTTP, so it runs through this same lock).
static SETTINGS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn settings_lock() -> &'static Mutex<()> {
    SETTINGS_LOCK.get_or_init(|| Mutex::new(()))
}

/// Read `~/.k2so/settings.json` and parse into `AppSettings`. Missing
/// or malformed files yield `AppSettings::default()`. Always
/// deep-merges with defaults so freshly-added keys are present in
/// returned values.
///
/// Legacy "projects" key is migrated in-memory to "projectSettings"
/// matching the pre-Unit-7a Tauri reader.
pub fn load() -> AppSettings {
    ensure_dir();
    let file = settings_file();
    if !file.exists() {
        return AppSettings::default();
    }
    restrict_mode_if_wider(&file);
    let raw = match fs::read_to_string(&file) {
        Ok(s) => s,
        Err(_) => return AppSettings::default(),
    };
    let mut parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return AppSettings::default(),
    };
    if let Some(obj) = parsed.as_object_mut() {
        if obj.contains_key("projects") && !obj.contains_key("projectSettings") {
            if let Some(v) = obj.remove("projects") {
                obj.insert("projectSettings".to_string(), v);
            }
        }
    }
    let mut defaults = serde_json::to_value(AppSettings::default()).unwrap_or_default();
    deep_merge(&mut defaults, &parsed);
    serde_json::from_value(defaults).unwrap_or_default()
}

/// Write `settings` to `~/.k2so/settings.json` via tmp+rename so a
/// concurrent reader never sees a partial file. Restricts the result
/// to mode 0o600 because the payload includes the legacy password
/// hash and the ngrok auth token.
pub fn save(settings: &AppSettings) -> Result<(), String> {
    ensure_dir();
    let file = settings_file();
    let tmp = file.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("serialize settings: {e}"))?;
    fs::write(&tmp, &json).map_err(|e| format!("write {tmp:?}: {e}"))?;
    fs::rename(&tmp, &file).map_err(|e| format!("rename {tmp:?} -> {file:?}: {e}"))?;
    restrict_mode(&file);
    Ok(())
}

/// Atomic load + deep-merge `partial` + save. Returns the post-merge
/// `AppSettings` so callers can echo it back to the renderer in one
/// round-trip.
///
/// **F3 closure:** if companion-affecting fields (`companion.username`,
/// `companion.passwordHash`, derived credentials) change between old
/// and new, calls
/// [`crate::companion::invalidate_all_sessions`] **after** the write
/// completes. The invalidation runs in the same process as the live
/// companion runtime — no two-writers race, no stale TTL window. The
/// emit is best-effort: a missing companion runtime (e.g. during
/// daemon boot before the autostart hook has fired) is a no-op inside
/// `invalidate_all_sessions`, which is exactly the correct behavior.
pub fn update(partial: serde_json::Value) -> Result<AppSettings, String> {
    let _guard = settings_lock().lock();
    let current = load();
    let mut current_val = serde_json::to_value(&current)
        .map_err(|e| format!("serialize current settings: {e}"))?;
    deep_merge(&mut current_val, &partial);
    let merged: AppSettings = serde_json::from_value(current_val)
        .map_err(|e| format!("deserialize merged settings: {e}"))?;

    let creds_changed = current.companion.password_hash != merged.companion.password_hash
        || current.companion.username != merged.companion.username;

    save(&merged)?;

    if creds_changed {
        crate::companion::invalidate_all_sessions("credentials changed");
    }

    Ok(merged)
}

/// Restore defaults: write `AppSettings::default()` over the on-disk
/// file, clear the macOS-Keychain password hash (so it can't outlive
/// a reset and re-activate later), and invalidate every live companion
/// session.
pub fn reset() -> Result<AppSettings, String> {
    let _guard = settings_lock().lock();
    let defaults = AppSettings::default();
    save(&defaults)?;

    // Best-effort Keychain delete. The helper swallows its own errors
    // (non-macOS dev / no `security` CLI) — a missing Keychain entry
    // also won't fail the reset, which matches the legacy Tauri
    // behavior.
    crate::companion::keychain::delete_password_hash();
    crate::companion::invalidate_all_sessions("settings reset");

    Ok(defaults)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test points HOME at a fresh tempdir so `~/.k2so/settings.json`
    /// is isolated from the developer's real settings file and from
    /// concurrent tests.
    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        _tmp: tempdir_lite::TempDir,
    }

    impl HomeGuard {
        fn new() -> Self {
            let tmp = tempdir_lite::TempDir::new("k2so-app-settings-test");
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", tmp.path());
            Self {
                original,
                _tmp: tmp,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    // Re-use the crate-wide HOME_LOCK from `themes` so the HOME-mutating
    // tests across modules (themes, skill_layers, chat_history, app_settings,
    // whats_new) all serialize on a single mutex. Cargo's parallel runner
    // would otherwise race separate per-module locks against each other
    // — they each guard their own module's tests but nothing stops two
    // modules from mutating `$HOME` simultaneously. Phase 2 Unit 6's
    // 68-test backfill (commit b65a5195) widened that pre-existing race
    // and broke `whats_new::tests::read_write_clear_state_roundtrips`
    // reliably; centralizing on one lock closes it.
    use crate::themes::HOME_LOCK as TEST_LOCK;

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let s = load();
        assert_eq!(s.default_agent, "claude");
        assert_eq!(s.terminal.font_size, 13);
    }

    #[test]
    fn save_then_load_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let mut s = AppSettings::default();
        s.default_agent = "codex".into();
        s.companion.username = "rosson".into();
        save(&s).expect("save");
        let loaded = load();
        assert_eq!(loaded.default_agent, "codex");
        assert_eq!(loaded.companion.username, "rosson");
    }

    #[test]
    fn update_deep_merges_partial() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let mut s = AppSettings::default();
        s.companion.username = "old".into();
        s.companion.ngrok_auth_token = "tok".into();
        save(&s).expect("save");
        let merged = update(serde_json::json!({
            "companion": { "username": "new" }
        }))
        .expect("update");
        // Username updates, ngrok_auth_token preserved by deep merge.
        assert_eq!(merged.companion.username, "new");
        assert_eq!(merged.companion.ngrok_auth_token, "tok");
    }

    #[test]
    fn reset_writes_defaults() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let mut s = AppSettings::default();
        s.default_agent = "codex".into();
        save(&s).expect("save");
        let after = reset().expect("reset");
        assert_eq!(after.default_agent, "claude");
        let loaded = load();
        assert_eq!(loaded.default_agent, "claude");
    }

    /// "Your display name" (composer `from` attribution) must persist
    /// through save→load and through an `update()` deep-merge, and default
    /// to `None` on a fresh settings file (→ the route's "owner" fallback).
    #[test]
    fn owner_display_name_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Fresh file: defaults to None (route falls back to "owner").
        assert_eq!(load().owner_display_name, None);

        // save → load preserves an explicit value.
        let mut s = AppSettings::default();
        s.owner_display_name = Some("Rosson".to_string());
        save(&s).expect("save");
        assert_eq!(load().owner_display_name.as_deref(), Some("Rosson"));

        // update() deep-merge ingests the camelCase key like any other
        // field (the generic /cli/settings/update path), no bespoke route.
        let merged = update(serde_json::json!({
            "ownerDisplayName": "Captain"
        }))
        .expect("update");
        assert_eq!(merged.owner_display_name.as_deref(), Some("Captain"));
        assert_eq!(load().owner_display_name.as_deref(), Some("Captain"));

        // reset() clears it back to None.
        let after = reset().expect("reset");
        assert_eq!(after.owner_display_name, None);
    }

    /// Composer 1c (D4) — the remote-instruct opt-in must default OFF on a
    /// fresh settings file, persist through save→load, and ingest the
    /// camelCase `allowRemoteInstruct` key via the generic `update()`
    /// deep-merge (the /cli/settings/update path the toggle hits). Default
    /// MUST be OFF: it gates connect-user RCE-grade agent instruction.
    #[test]
    fn allow_remote_instruct_defaults_off_and_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Fresh file: the security default is OFF.
        assert!(!load().allow_remote_instruct);
        assert!(!AppSettings::default().allow_remote_instruct);

        // save → load preserves an explicit opt-in.
        let mut s = AppSettings::default();
        s.allow_remote_instruct = true;
        save(&s).expect("save");
        assert!(load().allow_remote_instruct);

        // update() deep-merge ingests the camelCase key like any other field.
        let merged = update(serde_json::json!({
            "allowRemoteInstruct": false
        }))
        .expect("update");
        assert!(!merged.allow_remote_instruct);
        assert!(!load().allow_remote_instruct);

        // reset() returns to the OFF default.
        let after = reset().expect("reset");
        assert!(!after.allow_remote_instruct);
    }

    /// Remote Session Layer 0 — the remote-sessions master switch must
    /// default OFF on a fresh settings file, persist through save→load,
    /// and ingest the camelCase `remoteSessionsEnabled` key via the
    /// generic `update()` deep-merge (the /cli/settings/update path the
    /// toggle hits). Default MUST be OFF: fail-closed independent of grants.
    #[test]
    fn remote_sessions_enabled_defaults_off_and_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Fresh file: the security default is OFF.
        assert!(!load().remote_sessions_enabled);
        assert!(!AppSettings::default().remote_sessions_enabled);

        // save → load preserves an explicit opt-in.
        let mut s = AppSettings::default();
        s.remote_sessions_enabled = true;
        save(&s).expect("save");
        assert!(load().remote_sessions_enabled);

        // update() deep-merge ingests the camelCase key like any other field.
        let merged = update(serde_json::json!({
            "remoteSessionsEnabled": false
        }))
        .expect("update");
        assert!(!merged.remote_sessions_enabled);
        assert!(!load().remote_sessions_enabled);

        // reset() returns to the OFF default.
        let after = reset().expect("reset");
        assert!(!after.remote_sessions_enabled);
    }

    /// Hosted web client Layer 0 — the web-client master switch must
    /// **default ON** on a fresh settings file (product default, PRD §6.7),
    /// persist through save→load, and ingest the camelCase
    /// `webClientEnabled` key via the generic `update()` deep-merge.
    /// Unlike remote-sessions (default OFF), the web client is a headline
    /// feature; the toggle is for owners who want the browser door shut.
    #[test]
    fn web_client_enabled_defaults_on_and_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Fresh file: the product default is ON.
        assert!(load().web_client_enabled);
        assert!(AppSettings::default().web_client_enabled);

        // save → load preserves an explicit opt-out.
        let mut s = AppSettings::default();
        s.web_client_enabled = false;
        save(&s).expect("save");
        assert!(!load().web_client_enabled);

        // update() deep-merge ingests the camelCase key like any other field.
        let merged = update(serde_json::json!({
            "webClientEnabled": true
        }))
        .expect("update");
        assert!(merged.web_client_enabled);
        assert!(load().web_client_enabled);

        // reset() returns to the ON default.
        let _ = update(serde_json::json!({ "webClientEnabled": false })).expect("off");
        let after = reset().expect("reset");
        assert!(after.web_client_enabled);
        assert!(load().web_client_enabled);
    }

    /// DNS K1 — the dns-manage opt-in must default OFF on a fresh settings
    /// file, persist through save→load, and ingest the camelCase
    /// `dnsManageEnabled` key via the generic `update()` deep-merge (the
    /// /cli/settings/update path the toggle hits). Default MUST be OFF:
    /// it gates agent DNS mutation (high-impact zone writes).
    #[test]
    fn dns_manage_enabled_defaults_off_and_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Fresh file: the security default is OFF.
        assert!(!load().dns_manage_enabled);
        assert!(!AppSettings::default().dns_manage_enabled);

        // save → load preserves an explicit opt-in.
        let mut s = AppSettings::default();
        s.dns_manage_enabled = true;
        save(&s).expect("save");
        assert!(load().dns_manage_enabled);

        // update() deep-merge ingests the camelCase key like any other field.
        let merged = update(serde_json::json!({
            "dnsManageEnabled": false
        }))
        .expect("update");
        assert!(!merged.dns_manage_enabled);
        assert!(!load().dns_manage_enabled);

        // reset() returns to the OFF default.
        let after = reset().expect("reset");
        assert!(!after.dns_manage_enabled);
    }

    /// C1 (0.40.45) — agents-can-create-connections opt-in must default OFF,
    /// persist through save→load, and ingest camelCase
    /// `agentsCanCreateConnections` via `update()` deep-merge.
    #[test]
    fn agents_can_create_connections_defaults_off_and_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        assert!(!load().agents_can_create_connections);
        assert!(!AppSettings::default().agents_can_create_connections);

        let mut s = AppSettings::default();
        s.agents_can_create_connections = true;
        save(&s).expect("save");
        assert!(load().agents_can_create_connections);

        let merged = update(serde_json::json!({
            "agentsCanCreateConnections": false
        }))
        .expect("update");
        assert!(!merged.agents_can_create_connections);
        assert!(!load().agents_can_create_connections);

        let after = reset().expect("reset");
        assert!(!after.agents_can_create_connections);
    }

    /// Federation toggle-topology consolidation (UI-only move of the
    /// remote-instruct toggle to the K2 Connect page) — the MIGRATION is
    /// IDENTITY: neither key is renamed, no default changes, and the two
    /// flags stay independent, so every pre-update settings.json keeps its
    /// exact effective behavior. This test locks all four old flag
    /// combinations: a 0.40.x-shaped file (the same camelCase keys the
    /// /cli/settings/update deep-merge writes) must load back to the same
    /// pair, and updating one flag must never touch the other. A key
    /// rename or a coupled write would fail here loudly.
    #[test]
    fn federation_and_remote_instruct_flags_survive_update_unchanged() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        for (fed, instruct) in [(false, false), (false, true), (true, false), (true, true)] {
            // Persist the old-file shape via the same generic deep-merge
            // path the toggles hit.
            update(serde_json::json!({
                "federationEnabled": fed,
                "allowRemoteInstruct": instruct,
            }))
            .expect("update");
            let s = load();
            assert_eq!(
                s.federation_enabled, fed,
                "federationEnabled must round-trip unchanged for ({fed},{instruct})"
            );
            assert_eq!(
                s.allow_remote_instruct, instruct,
                "allowRemoteInstruct must round-trip unchanged for ({fed},{instruct})"
            );

            // Flipping ONE flag must leave the other exactly as persisted —
            // the toggles are independent gates (federation surface vs
            // delivery consent), never a coupled write.
            let merged = update(serde_json::json!({ "federationEnabled": !fed })).expect("update");
            assert_eq!(merged.federation_enabled, !fed);
            assert_eq!(
                merged.allow_remote_instruct, instruct,
                "toggling federationEnabled must not touch allowRemoteInstruct"
            );
            let merged =
                update(serde_json::json!({ "allowRemoteInstruct": !instruct })).expect("update");
            assert_eq!(merged.allow_remote_instruct, !instruct);
            assert_eq!(
                merged.federation_enabled, !fed,
                "toggling allowRemoteInstruct must not touch federationEnabled"
            );
        }
    }

    /// 0.40.43 (1c) — the public `/v1` API switch must default OFF on a
    /// fresh settings file, ingest the camelCase `apiEnabled` key via the
    /// generic `update()` deep-merge (the /cli/settings/update path the
    /// toggle hits), round-trip through save→load, and return to OFF on
    /// reset. Default MUST be OFF: the external `/v1` surface stays dark
    /// unless the owner (or the K2_API env flag) explicitly enables it.
    /// Also exercises the runtime mirror pair (`set_api_enabled` /
    /// `api_enabled_setting`) the daemon syncs at boot + on update.
    #[test]
    fn api_enabled_defaults_off_and_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        assert!(!load().api_enabled, "apiEnabled must default OFF");

        let merged = update(serde_json::json!({ "apiEnabled": true })).expect("update");
        assert!(merged.api_enabled, "update must ingest camelCase apiEnabled");
        assert!(load().api_enabled, "apiEnabled must persist through save→load");

        let after = reset().expect("reset");
        assert!(!after.api_enabled, "reset must return apiEnabled to OFF");

        // Runtime mirror: pure atomic, defaults OFF, flips both ways.
        assert!(!api_enabled_setting(), "mirror must default OFF");
        set_api_enabled(true);
        assert!(api_enabled_setting(), "mirror must reflect set(true)");
        set_api_enabled(false);
        assert!(!api_enabled_setting(), "mirror must reflect set(false)");
    }

    /// GH#8 — the "Use local LLM to detect HITL" opt-in must default OFF
    /// on a fresh settings file, persist through save→load, ingest the
    /// camelCase `useLlmHitlDetection` key via the generic `update()`
    /// deep-merge (the /cli/settings/update path the toggle hits), and
    /// return to OFF on reset. Default MUST be OFF: it is what keeps the
    /// 1.5B model from ever running during `talk` HITL detection unless
    /// the owner explicitly opts in.
    #[test]
    fn use_llm_hitl_detection_defaults_off_and_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Fresh file: the cost/battery default is OFF.
        assert!(!load().use_llm_hitl_detection);
        assert!(!AppSettings::default().use_llm_hitl_detection);

        // save → load preserves an explicit opt-in.
        let mut s = AppSettings::default();
        s.use_llm_hitl_detection = true;
        save(&s).expect("save");
        assert!(load().use_llm_hitl_detection);

        // update() deep-merge ingests the camelCase key like any other field.
        let merged = update(serde_json::json!({
            "useLlmHitlDetection": false
        }))
        .expect("update");
        assert!(!merged.use_llm_hitl_detection);
        assert!(!load().use_llm_hitl_detection);

        // reset() returns to the OFF default.
        let after = reset().expect("reset");
        assert!(!after.use_llm_hitl_detection);
    }

    #[test]
    fn legacy_projects_key_migrates_in_memory() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        ensure_dir();
        let raw = r#"{"projects": {"abc": {"foo": "bar"}}}"#;
        fs::write(settings_file(), raw).expect("write legacy");
        let s = load();
        assert!(s.project_settings.contains_key("abc"));
    }

    /// F3 closure smoke: when `update()` sees `companion.username`
    /// change, it must invoke `crate::companion::invalidate_all_sessions`.
    /// We can't easily start a real companion (needs ngrok + a
    /// listener), so we stuff a fake `Session` into the live
    /// `crate::companion::STATE` and assert that `update()` empties
    /// `state.sessions` for us.
    ///
    /// Pre-Unit-7a this was Tauri's job and the invalidate landed on
    /// an empty in-process state; the daemon-side `STATE` (where the
    /// live sessions actually live) stayed populated. This test pins
    /// down that the call now lands on the daemon-side `STATE` —
    /// which is exactly what closes the F3 gap.
    #[test]
    fn update_with_credential_change_invalidates_companion_sessions() {
        use crate::companion::types::{
            AuthRateLimiter, CompanionState, Session,
        };
        use parking_lot::Mutex as PlMutex;
        use std::collections::HashMap;
        use std::sync::atomic::AtomicBool;

        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Seed disk with a settings.json whose companion.username we
        // can rotate. Disk read happens inside `update()` to compute
        // the old-vs-new diff.
        let mut s = AppSettings::default();
        s.companion.username = "before".into();
        save(&s).expect("save initial");

        // Build a CompanionState with one fake session keyed "tok-A".
        // All fields private to types::CompanionState are public, so
        // we can construct one for the test without test-only helpers.
        let mut sessions = HashMap::new();
        let now = chrono::Utc::now();
        sessions.insert(
            "tok-A".to_string(),
            Session {
                token: "tok-A".to_string(),
                created_at: now,
                expires_at: now + chrono::Duration::hours(24),
                last_active: now,
                remote_addr: "127.0.0.1".to_string(),
                request_count: 0,
                window_start: std::time::Instant::now(),
            },
        );
        let fake_state = CompanionState {
            tunnel_url: PlMutex::new(None),
            sessions: PlMutex::new(sessions),
            ws_clients: PlMutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
            hook_port: 0,
            hook_token: String::new(),
            cors_origins: Vec::new(),
            allow_remote_spawn: false,
            auth_limiter: PlMutex::new(AuthRateLimiter::new()),
            reflow_cache: PlMutex::new(HashMap::new()),
            _tunnel_keepalive: PlMutex::new(None),
        };
        // Install the fake state in the live companion module.
        *crate::companion::STATE.lock() = Some(fake_state);

        // Rotate the username — F3-relevant field.
        let _ = update(serde_json::json!({
            "companion": { "username": "after" }
        }))
        .expect("update");

        // Sessions map should be empty post-update.
        let guard = crate::companion::STATE.lock();
        let state = guard.as_ref().expect("state installed above");
        assert!(
            state.sessions.lock().is_empty(),
            "F3 close: companion.sessions must be empty after a credential rotate"
        );

        // Cleanup so subsequent tests don't see a stale state.
        drop(guard);
        *crate::companion::STATE.lock() = None;
    }

    /// Tiny in-crate tempdir helper — we don't want a top-level
    /// `tempfile` dep for one usage. Mimics the minimal API we need:
    /// auto-create + auto-delete + path accessor.
    mod tempdir_lite {
        use std::path::{Path, PathBuf};

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn new(prefix: &str) -> Self {
                let pid = std::process::id();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let path =
                    std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}"));
                std::fs::create_dir_all(&path).expect("create tempdir");
                Self { path }
            }

            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
}
