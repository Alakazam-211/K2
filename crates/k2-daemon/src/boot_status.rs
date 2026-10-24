//! Daemon boot-phase state — the single source of truth behind the
//! `/boot-status` handshake and the dispatcher's readiness gate.
//!
//! ## Why this exists (0.39.5)
//!
//! Pre-0.39.5 the daemon ran every first-boot migration (the
//! 64-workspace unification + skills-consolidation + auto-pin sweep)
//! BEFORE it bound its port. During a 0.38.x → 0.39.x auto-update the
//! NEW daemon was therefore unreachable for the entire migration
//! window — while the OUTGOING old daemon was still bound to the stable
//! port and still answered `/ping` with 200. The renderer's
//! `ConnectionGate` took that false-positive "healthy" ping, mounted
//! the app, and its store fetches landed in the gap where the old
//! daemon had been killed and the new one was still migrating → blank
//! window ("appears to have crashed").
//!
//! The fix: bind the listener FIRST, advertise progress here, and have
//! the dispatcher 503 every real route until [`set_ready`] runs. The
//! renderer reads `phase` + `version` from `/boot-status` and only
//! mounts against a daemon whose version is paired with the app AND
//! whose phase is `ready` — so it can never bind to the outgoing old
//! daemon, and it can SHOW the user the migration is in progress.
//!
//! See `[[project_daemon_handshake_contract]]` and
//! `release-notes-0.39.5.md`.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};

/// daemon↔client API compatibility version. Bump this ONLY when a
/// change breaks the routes/contract clients depend on — NOT on every
/// release. K2 Connect range-checks `protocol` to decide whether it can
/// talk to a remote daemon of a different marketing version; the local
/// auto-update path keys off the exact `version` string instead. The
/// two are intentionally decoupled. Starts at 1.
pub const PROTOCOL: u32 = 1;

/// PRD boot-status-ghost §2.2: the OLDEST client↔daemon contract this
/// daemon still serves. `[PROTOCOL_MIN, PROTOCOL]` is the "clients that can
/// talk to me" range, readable before sign-in. Integers, not versions, so
/// they never fingerprint a build.
pub const PROTOCOL_MIN: u32 = 1;

// ── Views (PRD boot-status-ghost §2) ───────────────────────────────────

/// Which body `/boot-status` answers a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// No valid credential, non-loopback, `bootStatusPublic = ghost`:
    /// phase + protocol range + instanceId only.
    Public,
    /// No valid credential, non-loopback, `bootStatusPublic = compat` (the
    /// 0.46.3 default): the public view plus the five fields old phones and
    /// desktops read without a token.
    Compat,
    /// Loopback, or any valid owner / passport / API or status key /
    /// Connect-login session: today's body.
    Full,
}

impl View {
    pub fn as_wire(self) -> &'static str {
        match self {
            View::Public => "public",
            View::Compat => "compat",
            View::Full => "full",
        }
    }
}

/// True when the token is 64 lowercase hex chars (a Connect-login session's
/// shape). The format gate keeps a junk token from costing the session
/// store's file reads.
fn is_session_shaped(tok: &str) -> bool {
    tok.len() == 64 && tok.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Does `tok` earn the full body? Cheapest check first, stop at the first
/// hit (PRD §2.1): owner token → agent passport (`<sid>.<secret>`, in
/// memory) → `k2sk_` owner/status key (one indexed SQLite query) → a live,
/// unflagged Connect-login session (64 hex). A must-change-password session,
/// an App/guest pass (`k2skn_`), an edge signature, a revoked/disabled key
/// and anything else are NOT credentials here. Never argon2.
pub(crate) fn credential_earns_full(tok: &str, owner_token: &str) -> bool {
    let tok = tok.trim();
    if tok.is_empty() {
        return false;
    }
    if crate::routes::http::ct_eq_token(tok, owner_token) {
        return true;
    }
    if tok.starts_with(k2_core::api_keys::API_KEY_PREFIX) {
        return k2_core::api_keys::resolve_status_key(tok).is_some();
    }
    if tok.contains('.') {
        return crate::session_token::validate_hook(tok).is_some();
    }
    if is_session_shaped(tok) {
        return match k2_core::connect_users::validate_session(tok) {
            Some(user) => !k2_core::connect_users::must_change_password(&user),
            None => false,
        };
    }
    false
}

/// Pick the view for one `/boot-status` (or `/ping`) request. `token` is
/// the already-folded effective credential (Bearer > `?token=` > cookie,
/// `routes::http::effective_auth_query`). Loopback (after
/// `reclassify_proxied`) is always full; elsewhere a valid credential is
/// full, and no / a bad credential gets the configured public view. Never
/// 401/403.
pub(crate) fn view_for(
    ingress: crate::routes::dispatcher::Ingress,
    token: Option<&str>,
    owner_token: &str,
    public: k2_core::app_settings::BootStatusPublic,
) -> View {
    if ingress == crate::routes::dispatcher::Ingress::Loopback {
        return View::Full;
    }
    if token.is_some_and(|t| credential_earns_full(t, owner_token)) {
        return View::Full;
    }
    match public {
        k2_core::app_settings::BootStatusPublic::Compat => View::Compat,
        k2_core::app_settings::BootStatusPublic::Ghost => View::Public,
    }
}

/// Response header lines on every view: nothing between a client and the
/// box may ever cache a full body and serve it to a public caller.
pub(crate) const VIEW_HEADERS: &str = "Cache-Control: no-store\r\nVary: Authorization, Cookie\r\n";

/// The ghost body, exact key order (§2.2). A struct (not `json!`) so the
/// order is fixed whatever serde_json's map features are.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicBody<'a> {
    view: &'static str,
    phase: &'static str,
    protocol: u32,
    protocol_min: u32,
    instance_id: &'a str,
}

#[derive(serde::Serialize)]
struct AirgapEnabled {
    enabled: bool,
}

/// The compat body: the public view plus `version`, `features`,
/// `serverId`, `publicHost`, `airgap.enabled` (PRD Q9, B2).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CompatBody<'a> {
    view: &'static str,
    phase: &'static str,
    protocol: u32,
    protocol_min: u32,
    instance_id: &'a str,
    version: &'static str,
    features: Vec<&'static str>,
    server_id: Option<String>,
    public_host: Option<String>,
    airgap: AirgapEnabled,
}

/// The `/boot-status` body for `view`. `settings` is the one settings read
/// for this request.
pub(crate) fn body_for(view: View, settings: &k2_core::app_settings::AppSettings) -> String {
    match view {
        View::Public => serde_json::to_string(&PublicBody {
            view: View::Public.as_wire(),
            phase: phase_str(),
            protocol: PROTOCOL,
            protocol_min: PROTOCOL_MIN,
            instance_id: instance_id(),
        })
        .expect("serialize public boot-status"),
        View::Compat => serde_json::to_string(&CompatBody {
            view: View::Compat.as_wire(),
            phase: phase_str(),
            protocol: PROTOCOL,
            protocol_min: PROTOCOL_MIN,
            instance_id: instance_id(),
            version: env!("CARGO_PKG_VERSION"),
            features: features(),
            server_id: server_id(),
            public_host: public_host(),
            airgap: AirgapEnabled { enabled: k2_core::airgap::enabled() },
        })
        .expect("serialize compat boot-status"),
        // k2-ptyd P27: `ptyd: on | off (<reason>)` rides the full view only,
        // and only with the dev switch on (default body byte-identical).
        // 0.46.4 (update continuity R7/R8): `appEngine` (installed vs
        // running vs this K2, linger) only when an App engine is installed.
        View::Full => crate::apps::engine_heal::decorate_boot_status(crate::ptyd_status::decorate_boot_status(full_body(settings)))
            .to_string(),
    }
}

/// The `/ping` body: the version banner, except a caller that would get the
/// ghost `/boot-status` view gets `k2-daemon` with no version (§2.4).
pub(crate) fn ping_body(view: View) -> &'static str {
    match view {
        View::Public => PING_GHOST,
        View::Compat | View::Full => crate::BANNER,
    }
}

/// The ghost `/ping` banner (no version).
pub const PING_GHOST: &str = "k2-daemon";

/// Today's full body plus `view: "full"` and `protocolMin` (nothing
/// removed). Loopback and authed callers only.
fn full_body(settings: &k2_core::app_settings::AppSettings) -> serde_json::Value {
    serde_json::json!({
        "view": View::Full.as_wire(),
        "version": env!("CARGO_PKG_VERSION"),
        // daemon↔client API range: `protocol` is the newest contract this
        // daemon speaks, `protocolMin` the oldest it still serves.
        "protocol": PROTOCOL,
        "protocolMin": PROTOCOL_MIN,
        // starting | migrating | ready (+ reserved error). Clients treat
        // anything but `ready` as not-ready.
        "phase": phase_str(),
        // Free text for the UI only; never parsed.
        "detail": detail(),
        // 0.40.48 connection resilience: per-PROCESS instance id. Comparing
        // it across polls is how a client detects a silent daemon restart
        // (e.g. a self-update) and forces a cold resync.
        "instanceId": instance_id(),
        // homes-sync-v1 (0.46.1): this BOX's stable id; `publicHost` is the
        // tunnel host (`<sub>.k2.dev`) or null.
        "serverId": server_id(),
        "publicHost": public_host(),
        // Home M5: features the version string cannot tell apart. A client
        // reads an absent key as unsupported. See `features`.
        "features": features(),
        // 0.39.35: update SHAPE selector ("bundled-app" | "standalone").
        "installKind": install_kind(),
        // Hosted web client Layer 0 (PRD §6.7 / §9.4).
        "webClient": {
            "enabled": settings.web_client_enabled,
        },
        // COMPAT-58: the scoped-hook capability (feature-detect, no bump).
        "scopedHooks": {
            "supported": true,
            "enabled": crate::session_token::scoped_hooks_enabled(),
        },
        // Observability/Agent-Ops Phase E: read-only `/cli/ops/*`.
        "ops": {
            "supported": true,
            "version": "1",
        },
        // F3: the external `/v1` API capability.
        "api": crate::misc_routes::api_capability(),
        // Air-gap + LAN listen: hide-UI only (the daemon refuse is authority).
        "airgap": {
            "enabled": k2_core::airgap::enabled(),
            "baked": k2_core::airgap::baked(),
        },
        "listen": {
            "lan": k2_core::listen::lan_bound(),
        },
        // PRD connect-login-edge-only I6: the tunnel password-login mode.
        "connectLogin": {
            "ingress": settings.connect_login_ingress().as_wire(),
        },
    })
}

/// homes-sync-v1: this box's stable `serverId` (the first 16 hex of the
/// SHA-256 of its federation identity public key,
/// `k2_core::federation::identity::server_id`). Unlike [`instance_id`] it
/// survives restarts. Cached per identity path (tests move `$HOME`); `None`
/// only when the identity key can't be read or made.
pub fn server_id() -> Option<String> {
    static CACHE: std::sync::Mutex<Option<(std::path::PathBuf, String)>> = std::sync::Mutex::new(None);
    let path = k2_core::federation::identity::identity_path();
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((p, id)) = cache.as_ref() {
        if *p == path && path.exists() {
            return Some(id.clone());
        }
    }
    match k2_core::federation::identity::server_id() {
        Ok(id) => {
            *cache = Some((path, id.clone()));
            Some(id)
        }
        Err(e) => {
            eprintln!("[boot-status] serverId unavailable: {e}");
            None
        }
    }
}

/// homes-sync-v1: this box's public host key (`<sub>.k2.dev`) when a
/// tunnel is configured with a token, else `None`.
pub fn public_host() -> Option<String> {
    let cfg = k2_core::tunnel::config::load().ok().filter(|c| !c.token.trim().is_empty())?;
    let url = cfg.public_url()?;
    url.strip_prefix("https://").map(|h| h.trim_end_matches('/').to_ascii_lowercase())
}

/// Per-process daemon instance id — 16 lowercase hex chars minted from the
/// OS CSPRNG the first time it's read, then stable for the life of the
/// process. NEVER persisted: a new id on every daemon start is the whole
/// point. Clients (0.40.48 connection resilience) compare the id across a
/// reconnect — `/boot-status` polls and the session-events `hello` frame
/// both carry it — to detect that the daemon RESTARTED underneath them
/// (h2-coalesced connections can look "alive" across a restart) and force
/// a full re-reconcile instead of trusting stale subscriptions.
pub fn instance_id() -> &'static str {
    static INSTANCE_ID: OnceLock<String> = OnceLock::new();
    INSTANCE_ID.get_or_init(|| {
        let mut bytes = [0u8; 8];
        // Same CSPRNG the hook-token secret uses (session_token.rs);
        // an unusable OS RNG is unrecoverable, so fail loudly like it does.
        getrandom::getrandom(&mut bytes).expect("getrandom for daemon instance id");
        let mut s = String::with_capacity(16);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    })
}

const STARTING: u8 = 0;
const MIGRATING: u8 = 1;
const READY: u8 = 2;
const ERROR: u8 = 3;

/// The phase machine behind `/boot-status`. The daemon has exactly one
/// ([`BOOT`], driven by the free functions below); tests build their own
/// so they never flip the process-wide readiness gate that every
/// in-process HTTP test depends on (the test daemon harness calls
/// [`set_ready`] once and expects it to stay set).
struct BootState {
    phase: AtomicU8,
    detail: RwLock<String>,
}

impl BootState {
    const fn new() -> Self {
        Self { phase: AtomicU8::new(STARTING), detail: RwLock::new(String::new()) }
    }

    fn set_migrating(&self, detail: &str) {
        self.set_detail(detail);
        self.phase.store(MIGRATING, Ordering::SeqCst);
    }

    fn set_detail(&self, detail: &str) {
        if let Ok(mut d) = self.detail.write() {
            *d = detail.to_string();
        }
    }

    fn set_ready(&self) {
        self.set_detail("");
        self.phase.store(READY, Ordering::SeqCst);
    }

    fn set_error(&self, detail: &str) {
        self.set_detail(detail);
        self.phase.store(ERROR, Ordering::SeqCst);
    }

    fn is_ready(&self) -> bool {
        self.phase.load(Ordering::SeqCst) == READY
    }

    fn phase_str(&self) -> &'static str {
        match self.phase.load(Ordering::SeqCst) {
            MIGRATING => "migrating",
            READY => "ready",
            ERROR => "error",
            _ => "starting",
        }
    }

    fn detail(&self) -> String {
        match self.detail.read() {
            Ok(d) => d.clone(),
            // Poisoned lock should never happen (no panics under the lock),
            // but degrade to an empty detail rather than taking the daemon
            // down over a status string.
            Err(_) => String::new(),
        }
    }
}

/// This daemon's boot phase.
static BOOT: BootState = BootState::new();

/// Enter the `migrating` phase with an initial human-readable detail.
pub fn set_migrating(detail: &str) {
    BOOT.set_migrating(detail);
}

/// Update the human-readable detail string. UI-only — clients MUST NOT
/// parse this for logic; it exists purely so the window can show
/// "Applying updates… (12/64 workspaces)" while we work.
pub fn set_detail(detail: &str) {
    BOOT.set_detail(detail);
}

/// Mark the daemon fully booted: migrations done, every provider/sink
/// registered, real routes safe to serve. Clears the detail. This is
/// the gate the dispatcher and the renderer both wait on.
pub fn set_ready() {
    BOOT.set_ready();
}

/// Mark a fatal boot/migration error with a human-readable detail.
/// Reserved for future wiring (e.g. a migration that hard-fails) so the
/// renderer can surface a real error instead of spinning forever.
#[allow(dead_code)]
pub fn set_error(detail: &str) {
    BOOT.set_error(detail);
}

/// True once [`set_ready`] has run. The dispatcher uses this to 503
/// every non-liveness route until first-boot migrations complete.
pub fn is_ready() -> bool {
    BOOT.is_ready()
}

/// Lowercase phase string for the `/boot-status` JSON. Unknown future
/// values never appear here, but clients should treat any phase other
/// than `ready` as "not ready" (forward-compatible).
pub fn phase_str() -> &'static str {
    BOOT.phase_str()
}

/// Current human-readable detail (cloned). UI-only.
pub fn detail() -> String {
    BOOT.detail()
}

// ─────────────────────────────────────────────────────────────────────
// Install-kind classification (0.39.35 — unified remote update)
// ─────────────────────────────────────────────────────────────────────

/// How THIS daemon binary was installed, which decides the update SHAPE:
///
///   - `"bundled-app"` — the daemon binary lives INSIDE a macOS `.app`
///     bundle (its path contains `.app/Contents/`). Updating means
///     replacing the whole signed/notarized bundle, which only the
///     co-located app's Tauri updater can do (Shape A). The daemon
///     remote-triggers that app rather than swapping its own binary.
///   - `"standalone"` — a bare `k2so-daemon` binary under a supervisor
///     (launchd/systemd). Updating is the in-daemon download→verify→
///     stage→swap path (Shape B).
///   - `"package"` (0.46.3) — a package manager installed the binary
///     (`/usr/bin/k2-daemon` from the Arch package or a .deb, or a Nix
///     store path). The folder isn't the daemon's to write and the package
///     manager owns the file, so the daemon never updates itself: `check`
///     reports `available: false` (+ `newerViaPackage`), `start` / `apply`
///     refuse with a clear message. Was reported as `"standalone"` before,
///     and `apply` then failed with "Permission denied".
///   - `"unknown"` — the path couldn't be resolved; callers treat this
///     conservatively (no remote update shape is assumed).
///
/// This is reported on `/boot-status` and in the `update/check`
/// CheckResult so the renderer can vary copy, and `update/start` routes
/// on it to pick Shape A vs Shape B.
pub fn install_kind() -> &'static str {
    let exe = std::env::current_exe().ok();
    classify_install_kind(exe.as_deref())
}

/// Pure classifier behind [`install_kind`], split out so it's unit-
/// testable without depending on the test binary's own path. Returns one
/// of `"bundled-app" | "package" | "standalone" | "unknown"`.
pub fn classify_install_kind(exe: Option<&std::path::Path>) -> &'static str {
    let Some(exe) = exe else {
        return "unknown";
    };
    // A macOS app bundle nests the executable at
    // `…/K2.app/Contents/MacOS/<bin>` (or, for the daemon since 0.43.2,
    // `…/K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon`).
    // Detect the `.app/Contents/`
    // segment anywhere in the path — robust to the bundle name and to a
    // sidecar daemon binary placed elsewhere under Contents/.
    let s = exe.to_string_lossy();
    if s.contains(".app/Contents/") {
        return "bundled-app";
    }
    if is_package_path(&s) {
        return INSTALL_KIND_PACKAGE;
    }
    "standalone"
}

/// `installKind` of a package-manager install (0.46.3).
pub const INSTALL_KIND_PACKAGE: &str = "package";

/// True for a path a package manager owns: under `/usr/` but not
/// `/usr/local/` (FHS: `/usr` is the distribution's, `/usr/local` the
/// admin's — where the standalone installers put K2), or the Nix store.
fn is_package_path(s: &str) -> bool {
    (s.starts_with("/usr/") && !s.starts_with("/usr/local/")) || s.starts_with("/nix/store/")
}

/// Home M4/M5: `sessions/v2/spawn` honours `attach_only` (a live session
/// is returned as-is; anything else is `404 session_not_live`). Released
/// daemons up to 0.41.6 ignore unknown body fields and would SPAWN, and the
/// version string cannot tell this build from 0.41.6, so clients read this
/// key from `/boot-status` `features` instead of the version.
pub const FEATURE_SPAWN_ATTACH_ONLY: &str = "spawn-attach-only";

/// Test hook (debug builds only): `K2_TEST_SIMULATE_NO_ATTACH_ONLY=1` makes
/// this daemon behave like a released one up to 0.41.6 for the two-daemon
/// harness: it does not report [`FEATURE_SPAWN_ATTACH_ONLY`] and
/// `sessions/v2/spawn` ignores `attach_only`. A release build ignores it.
pub fn attach_only_supported() -> bool {
    !(cfg!(debug_assertions)
        && std::env::var("K2_TEST_SIMULATE_NO_ATTACH_ONLY").as_deref() == Ok("1"))
}

/// `GET /cli/feedback/list-all` exists and `waiting-count` counts only
/// tickets on a registered workspace (prd-tickets-badge-orphans TB14).
/// A client reads this key, not the version: main and the last release
/// share a version string until the next cut.
pub const FEATURE_TICKETS_LIST_ALL: &str = "tickets-list-all";

/// `GET /cli/feedback/waiting-count` counts `waiting` + `needs_discussion`
/// (the board's "Waiting on me" statuses) and honours `me=1`: only tickets
/// assigned to the calling login (owner token → `owner` + the owner's
/// display name; a Connect-user session → its username). A client without
/// the key keeps the host-wide count and never sends `me=1`.
pub const FEATURE_TICKETS_BADGE_ME: &str = "tickets-badge-me";

/// Zen Mode v1 (prd-zen-mode-v1 Z16): this daemon serves `/cli/zen/*`
/// for the person on this computer (owner token only).
pub const FEATURE_ZEN_V1: &str = "zen-v1";

/// Zen Gardens (prd-zen-gardens-v1 G19): `/cli/zen/setup`, `/cli/zen/gardens`,
/// `/cli/zen/garden/*` and `get?garden=` exist. The renderer requires it
/// from the LOCAL scope; without it Zen shows safe mode ("K2 on this
/// computer is older than this app").
pub const FEATURE_ZEN_GARDENS: &str = "zen-gardens-v1";

/// Zen freeform chrome (prd-zen-freeform-chrome FC29): `GET /cli/zen/get`
/// answers `page.chrome`, `page.bands`, `page.edges` and `page.menus`, and a
/// Garden file may place K2's controls (`slot` `bottom`/`menu`, `align`,
/// `edge`, `menu`). A client without it draws the template's chrome.
pub const FEATURE_ZEN_CHROME: &str = "zen-chrome-v1";

/// Zen v2 custom widgets (prd-zen-user-widgets-v2 UW36): `kind = "custom"`
/// placements in `GET /cli/zen/get`, widget folders and bundles
/// (`/cli/zen/widgets`, `widget/bundle`, `widget/new`), the runaway pause
/// (`widget/pause|resume`) and `GET /cli/zen/templates`. Widgets in your own
/// Garden need no grants (2026-10-08).
pub const FEATURE_ZEN_WIDGETS: &str = "zen-widgets-v1";

/// Zen v2 widget library downloads (UWB15): `/cli/zen/lib/fetch` and
/// `/cli/zen/lib/file` (`zen_lib_routes`, B3).
pub const FEATURE_ZEN_LIB: &str = crate::zen_lib_routes::FEATURE;

/// A Thread post from a Garden widget carries `origin: {widget, garden}`;
/// the daemon stores it and skips compose history (UWB12a, B3's
/// `overlay_routes`).
pub const FEATURE_THREAD_WIDGET_ORIGIN: &str = "thread-widget-origin";

/// Zen Garden sync (prd-zen-garden-sync-defaults-v1 GS31):
/// `/cli/zen/sync`, `/cli/zen/garden/sync`, `/cli/zen/news`,
/// `/cli/zen/news/seen`, `get?preview=`, and `sync` + `frame` in
/// `GET /cli/zen/get`. Without it the app shows no sync switches and no
/// Garden news card.
pub const FEATURE_ZEN_SYNC: &str = "zen-sync-v1";

/// Garden Lock (prd-garden-lock-and-sync-v1): `POST /cli/zen/garden/lock`,
/// `locked` on `/cli/zen/sync` and `/cli/zen/gardens` rows, 423
/// `garden_locked` for an agent's change to a locked Garden, and
/// `widget/bundle?garden=` serving a locked Garden's frozen widgets.
pub const FEATURE_ZEN_GARDEN_LOCK: &str = "zen-garden-lock-v1";

/// Zen Z41: `GET /cli/thread/latest?addrs=` answers on this server. A
/// client without it falls back to `GET /cli/thread?addr=&limit=1`.
pub const FEATURE_THREAD_LATEST: &str = "thread-latest";

/// prd-home-picker-and-remote-avatars-v1 S3: this daemon serves the
/// local-only Home avatar cache (`/cli/home/avatars*`).
pub const FEATURE_HOME_AVATARS: &str = "home-avatars-v1";

/// prd-daemon-activity-and-thread-working-v1 RL8 / A34: this daemon owns
/// agent activity (`activity_changed` + `GET /cli/activity/snapshot`). A
/// client renders its rows; a server without the key gets RL13.
pub const FEATURE_DAEMON_ACTIVITY: &str = "daemon-activity";

/// prd-thread-survives-tab-rename-v1 S3: a sidecar's old addresses keep
/// delivering after a rename (`movedFrom` on Thread routes), and the
/// daemon emits `session_address_changed` + the overlay `address` frame.
/// A client without it re-looks-up the address on a Thread 404.
pub const FEATURE_THREAD_ADDRESS_STABLE: &str = "thread-address-stable";

/// prd-thread-push-notifications-v1 §5.6: the `/cli/notify/*` routes, the
/// Thread push trigger and the §7 payload are live on this server. On only
/// when migration 0157's tables exist. It does NOT mean the relay is
/// reachable (`GET /cli/notify/prefs` `push` says that).
pub const FEATURE_THREAD_PUSH: &str = "thread-push-v1";

/// Garden v3 data sources (contract-sources-v1 §6): the `sources.*` widget
/// verbs' catalog feature. `k2.can('sources.read')` reads it from this
/// computer's daemon.
pub const FEATURE_SOURCES: &str = "sources-v1";

/// Local Grok Bots (0.45.7): this daemon serves `/cli/grokbots/*` and
/// emits `grokbots_changed`. A client hides Grok Bot rows on a server
/// without it.
pub const FEATURE_GROKBOTS: &str = "grokbots-v1";

/// Garden data (contract-garden-data-v1 §7): the `store.*`, `save.*` and
/// `viewer.get` garden-local verbs' catalog feature, served through
/// `POST /cli/zen/call` on this computer's daemon.
pub const FEATURE_GARDEN_DATA: &str = "garden-data-v1";

/// The App API (runtime S23, room-scope R1/R5/R6, 0.46.1): the App-facing
/// routes are catalogued (`contract/catalog.json` rows exposed to `app`;
/// the gateway table is generated from them), `GET /cli/skin/me` answers,
/// and a guest or App pass may read `GET /cli/thread/latest` for its rooms.
pub const FEATURE_APP_API: &str = "app-api-v1";

/// RS-A15 `guests:rooms` (0.46.1): an App with the owner's `guests-rooms`
/// grant reads and sets its own guests' rooms with its own pass
/// (`/cli/skin/guests/rooms`), and a Garden widget does the same for any
/// local App through `/cli/zen/call`, each write behind K2's confirm sheet.
pub const FEATURE_GUESTS_ROOMS: &str = "guests-rooms-v1";
/// Room-scope R9 (0.46.1): `room.members` on the App gateway
/// (`GET /cli/skin/members`, cap `room:members`) and the Garden bridge.
pub const FEATURE_ROOM_MEMBERS: &str = crate::apps::surfaces::FEATURE_ROOM_MEMBERS;

/// Room-scope R16 (0.46.1): `app.self` / `app.usage` on the App's link and
/// the Garden bridge, and the Garden's `widget.self`.
pub const FEATURE_APP_SELF: &str = crate::apps::surfaces::FEATURE_APP_SELF;
/// Folder path scope per room line (room-scope R3, RS-A1..A3, 0.46.1):
/// a role's room line may list read/write folders and an `uploadDir`
/// (`k2 app role paths`), every guest file door and `fs/events` honour it,
/// `skin/me` rooms carry `folders`, and `GET /cli/fs/stat` (`files.stat`)
/// answers.
pub const FEATURE_FILES_PATH_SCOPE: &str = "files-path-scope-v1";
/// Delta reads for an on-device chat cache (thread-delta-v1, 0.46.1):
/// `GET /cli/thread?after_seq=N` and `GET /cli/project-group/messages?
/// after_seq=N` return items with `seq > N` OLDEST first, `has_more` when
/// newer ones remain past `limit`; the Thread one also returns `changed`
/// (items at or below N rewritten in place since: card answers, voids,
/// expiries, handoffs) and `removed` (items that left the Thread).
pub const FEATURE_THREAD_DELTA: &str = "thread-delta-v1";
/// The Board context chip (thread-context-v1, 0.46.3 r10,
/// prd-garden-column-collapse-and-agent-list-v1 §4): `POST /cli/thread/post`
/// (unbound `via=compose`) takes `context: {text, origin}`, checks the
/// origin (the widget's `thread:post`, live hash) and the text (one line,
/// ≤ 120 characters, no stamp markers), stores it as the doc's own `context`
/// field and gives the agent K2's own quoted context line before the
/// `[from <user>]` stamp. A client sends a context only to a server with
/// this key; an older one gets the message without it.
pub const FEATURE_THREAD_CONTEXT: &str = "thread-context-v1";
/// 0.46.4 (prd-app-resources-disk-guard-v1 Slice B): the garden-local
/// `app.resources` verb (box usage's Garden card, `k2:box-disk@1`).
pub const FEATURE_APP_RESOURCES: &str = "app-resources-v1";
/// Older pages of a project chat (project-chat-paging-v1, 0.46.2):
/// `GET /cli/project-group/messages?before_seq=N&limit=L` returns the newest
/// `L` messages with `seq < N` OLDEST first, `has_more` when older ones
/// remain (the `/cli/thread` `before_seq` page). A bad `before_seq` or
/// `limit`, or `before_seq` with `after_seq` / `after`, is 400 `usage`.
/// K2 Companion pages a project chat back only on a server with this key.
pub const FEATURE_PROJECT_CHAT_PAGING: &str = "project-chat-paging-v1";
/// Project chat for Apps (0.46.3, App only, never the Garden bridge):
/// `project.list` / `project.read` / `project.post` / `project.markRead`
/// (`/cli/skin/projects*`) and `project.events` (`WS
/// /cli/skin/projects/events?group=`). A pass sees a project only when every
/// member agent is a room it holds with `thread:read`; posting also needs
/// `project:write` in every member room.
pub const FEATURE_APP_PROJECT_CHAT: &str = crate::apps::project_chat::FEATURE;
/// App users' personal wallets (0.46.3): the `wallet.*` verbs
/// (`/cli/skin/wallet/*`, a guest's own session pass, cap `wallet:self`)
/// and the owner's `/cli/app/wallet/*`.
pub const FEATURE_APP_WALLET: &str = "app-wallet-v1";
/// Thread push message previews (0.46.1): `POST /cli/push/register-device`
/// takes `previews` (absent = keep the stored value), its reply and
/// `GET /cli/push/device` carry `previews` + `previewsAllowed`, and a
/// Thread push body may be a message preview. K2 Companion sends
/// `previews` only to a server with this key.
pub const FEATURE_PUSH_PREVIEWS: &str = "push-previews-v1";
/// End-to-end encrypted pushes (prd-e2e-push-v1 rev 2, 0.46.3, migration
/// 0211): `POST /cli/push/register-device` takes `e2e` (the phone's P-256
/// public key + PSK; absent = keep, `null` = delete), and its reply and
/// `GET /cli/push/device` carry `e2e` (or `null`) and no vendor token.
/// K2 Companion sends `e2e` only to a server with this key.
pub const FEATURE_PUSH_E2E: &str = "push-e2e-v1";

/// Unread counts (thread-read-v1, 0.46.1, migration 0190): per-person read
/// cursors and mutes for Threads and project chats — `GET
/// /cli/thread/unread`, `POST /cli/thread/read`, `POST
/// /cli/project-group/read`, `POST /cli/thread/mute`; `unread` /
/// `waiting` / `muted` on `/cli/thread/latest` rows and the project list;
/// person-scoped `thread_read` / `project_read` frames on
/// `/cli/sessions/events`; `unreadTotal` + `waitingTotal` in Thread and
/// project push data; silent `kind: "badge"` pushes. K2 Companion shows
/// no dots and leaves a server out of its badge without this key.
pub const FEATURE_THREAD_READ: &str = "thread-read-v1";

/// File objects on Thread messages (0.46.1, S1 + S2): `attachments` on
/// Thread docs (`GET /cli/thread`, overlay WS frames, the
/// `POST /cli/thread/post` reply), `attachmentCount` on
/// `/cli/thread/latest` rows, and `GET /cli/thread/attachment`
/// (meta / preview / range). K2 Companion shows no file chips without it.
/// Stored in the overlay redb docs: no SQL migration.
pub const FEATURE_THREAD_ATTACHMENTS: &str = crate::thread_attachments::FEATURE;

/// Thread images (0.46.3, S3): `mode=thumb&px=256|512` on
/// `GET /cli/thread/attachment` for PNG, JPEG, GIF, WebP, BMP, TIFF,
/// HEIC/HEIF (where the box has a converter; else 415 `no_thumbnail`) and
/// SVG (always the sanitized rewrite, also on `mode=preview`). Clients
/// show image attachments as images only with this key.
pub const FEATURE_THREAD_IMAGES: &str = crate::thread_attachments::FEATURE_IMAGES;

/// App gateway Thread parity (0.46.3, Rosson 2026-10-20): App guests and
/// App passes get `GET /cli/thread/attachment`, `POST /cli/thread/catchup`,
/// their own read cursors (`POST /cli/thread/read`, `GET
/// /cli/thread/unread`), `POST /cli/fs/resolve-links` under their file
/// rights, and attachments on `POST /cli/thread/post` for files their own
/// file rights cover. Catalog v16 rows.
pub const FEATURE_APP_THREAD_PARITY: &str = "app-thread-parity-v1";

/// App read state per person (0.46.3; Rosson 2026-10-20: read state
/// belongs to the signed-in person): an App guest's read state is keyed by
/// the person, `guest:<principalId>`, on every App, door, device and
/// session (`read_routes::PassReader`); `GET /cli/thread/unread` for a
/// guest carries its `badgeKey` and `muted`, totals count waiting cards
/// and only its own rooms (R3); `POST /cli/thread/mute` takes a guest's
/// pass (catalog `thread.mute`); and a guest's overlay and activity
/// sockets get a `read` frame when its own cursor or mute moves on any
/// surface. An App's own pass is a service account, not a person: no
/// badgeKey, no mute.
pub const FEATURE_APP_READ_STATE: &str = "app-read-state-v1";

/// The Garden bridge's person rows (0.46.3, research I): a Garden widget
/// on this computer calls `thread.catchup`, `thread.unread`,
/// `thread.mute`, `tickets.list/show/create/comment/resolve`,
/// `activity.snapshot` and `files.list/read/readBinary` through `POST
/// /cli/zen/call`, as the person (an agent-made widget: its maker's own
/// room for tickets, activity and files). The same catalog rows and
/// answers as the App's; no project chat on the bridge.
pub const FEATURE_GARDEN_PERSON_ROWS: &str = "garden-person-rows-v1";

/// True when the read-state tables exist in the daemon DB.
fn thread_read_ready() -> bool {
    k2_core::db::try_shared()
        .map(|db| k2_core::read_state::tables_exist(&db.lock()))
        .unwrap_or(false)
}

/// homes-sync-v1 (0.46.1): `/cli/me/homes*` (a person's Homes synced
/// between their devices), `homes_changed` / `homes_device_removed` frames
/// and `serverId` on `/boot-status`. On only when migration 0193's tables
/// exist.
pub const FEATURE_HOMES_SYNC: &str = k2_core::homes_sync::FEATURE;

fn homes_sync_ready() -> bool {
    k2_core::db::try_shared()
        .map(|db| k2_core::homes_sync::tables_exist(&db.lock()))
        .unwrap_or(false)
}

/// Phone inbox catch-up (inbox-catchup-v1, 0.46.3): `POST
/// /cli/thread/catchup` (Thread deltas or newest pages for up to 200
/// chats plus up to 50 project chats in one call, each entry exactly the
/// plain read's body for the same caller), and top-level `rev` +
/// `instanceId` on every `GET /cli/thread` answer (and `rev` on overlay
/// WS frames). No SQL migration.
pub const FEATURE_INBOX_CATCHUP: &str = "inbox-catchup-v1";
/// `POST /cli/thread/catchup` takes optional `sinceRev` + `instanceId` and
/// answers `{ok, rev, instanceId, unchanged: true}` with no reads when no
/// Thread changed since (0.46.3). Threads only: project chats still need a
/// normal call.
pub const FEATURE_CATCHUP_UNCHANGED: &str = "catchup-unchanged-v1";

/// App mobile access (app-mobile-access-v1, 0.46.3): the agent access
/// list on `GET /cli/skin/me` (`agents`, `serverId`, `publicHost`,
/// `instanceId`), device sign-in (`device` on login, `POST
/// /cli/skin/refresh`, `/cli/skin/devices*`), per-App discovery (`GET
/// /cli/skin/app-info`) and the main-port catalog gate for App passes.
/// skin.db tables only, no SQL migration.
pub const FEATURE_APP_MOBILE_ACCESS: &str = "app-mobile-access-v1";

/// phone-web-off (2026-10-24): Companion phone pairing. `POST
/// /cli/auth/device/pair-code` (owner token or a login, never a browser),
/// `POST /cli/auth/device/pair {code, device}` (public, open while web
/// access is off) answering a device session that passes the web-access
/// wall, `GET /cli/auth/devices`, `POST /cli/auth/device/revoke`. No SQL
/// migration (the Connect session file carries the device binding).
pub const FEATURE_COMPANION_PAIRING: &str = "companion-pairing-v1";

/// One live Thread stream across rooms (app-room-stream, 0.46.3):
/// `WS /cli/thread/stream?rooms=all|<ids>` for an App pass (helper, link,
/// main port) or a login, frames in the inbox stream format
/// (`thread.stream` on the App surface and the Garden bridge). No SQL
/// migration.
pub const FEATURE_THREAD_STREAM: &str = "thread-stream-v1";

/// The counts-only activity ear for App guests (0.46.3, Rosson
/// 2026-10-21): with `thread:read` AND `activity:read` in a room, the
/// pinned-Chat Thread turn's guest projection on `WS /cli/overlay/events`
/// (`collection: "activity"`) and `WS /cli/thread/stream` (`kind:
/// "thread_activity"`), both opted in with `?ear=1`, and `GET
/// /cli/thread/activity` (`thread.activity`). `activity:read` still gives
/// the room-wide counts too, as before. No SQL migration.
pub const FEATURE_THREAD_ACTIVITY_EAR: &str = "thread-activity-ear-v1";

/// plan-agent-status-unify-v1 D1/D2 (0.46.0): the activity rollup carries
/// `live` and `busy` (snapshot `workspaces[]` and every `activity_changed`
/// `workspace`), and `presence/summary` `agentActivity` entries carry
/// `heartbeat`, `live`, `busy` and `counts` (idle workspaces with a live
/// PTY or a heartbeat listed too). A client drops its own live-session
/// poll (`liveSessionCwds`) only on a server that reports this key.
pub const FEATURE_ACTIVITY_LIVE_BUSY: &str = "activity-live-busy";

/// Per-agent unread and ticket counts (0.46.4, Rosson 2026-10-24):
/// `presence/summary` `agentActivity` entries carry the caller's `unread`,
/// `waiting`, `muted` on that agent's Threads and the agent's
/// `ticketsWaiting` / `ticketsWaitingMe` (workspaces with any of these
/// listed even when idle), and the App surface's agent rows
/// (`GET /cli/skin/agents` `roomAgents.list`, `GET /cli/skin/me`
/// `agents[]`) carry `unread` / `waiting` / `muted` where the pass holds
/// `thread:read` in the room and `ticketsWaiting` where it holds
/// `tickets:read`. Listed with the read-state tables (migration 0190). A
/// client without the key shows no remote row badges (never guesses).
pub const FEATURE_AGENT_UNREAD_TICKETS: &str = "agent-unread-tickets-v1";

/// `GET /cli/llm/accounts/for?addr=` answers the LLM token one chat (the
/// pinned chat or a sidecar `ws/N`) runs on, with that token's chip number
/// (`maxPercent`, the desktop top-bar chip's own calculation, also served
/// on each `/cli/usage/subscriptions` row), and `POST
/// /cli/llm/accounts/for/refresh` re-reads that one token (once a minute).
/// The phone shows it in a chat header and never probes an older server.
pub const FEATURE_LLM_ACCOUNT_FOR: &str = "llm-account-for-v1";

/// True when the notify prefs tables exist in the daemon DB.
fn thread_push_ready() -> bool {
    k2_core::db::try_shared()
        .map(|db| k2_core::notify_prefs::tables_exist(&db.lock()))
        .unwrap_or(false)
}

/// Migration 0211 ran: `push_device_keys` exists.
fn push_e2e_ready() -> bool {
    k2_core::db::try_shared()
        .map(|db| {
            db.lock()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'push_device_keys'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n > 0)
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Client-visible features this daemon has that its version string cannot
/// tell apart (`/boot-status` `features`). A client treats a key that is
/// absent — or a daemon with no `features` at all — as unsupported.
pub fn features() -> Vec<&'static str> {
    let mut out = Vec::new();
    if attach_only_supported() {
        out.push(FEATURE_SPAWN_ATTACH_ONLY);
    }
    out.push(FEATURE_TICKETS_LIST_ALL);
    out.push(FEATURE_TICKETS_BADGE_ME);
    out.push(FEATURE_ZEN_V1);
    out.push(FEATURE_ZEN_GARDENS);
    out.push(FEATURE_ZEN_CHROME);
    out.push(FEATURE_ZEN_WIDGETS);
    out.push(FEATURE_ZEN_LIB);
    out.push(FEATURE_ZEN_SYNC);
    out.push(FEATURE_ZEN_GARDEN_LOCK);
    out.push(FEATURE_THREAD_LATEST);
    out.push(FEATURE_THREAD_WIDGET_ORIGIN);
    out.push(FEATURE_HOME_AVATARS);
    out.push(FEATURE_DAEMON_ACTIVITY);
    out.push(FEATURE_THREAD_ADDRESS_STABLE);
    out.push(FEATURE_SOURCES);
    out.push(FEATURE_GARDEN_DATA);
    out.push(FEATURE_GROKBOTS);
    out.push(FEATURE_APP_API);
    out.push(FEATURE_GUESTS_ROOMS);
    out.push(FEATURE_ROOM_MEMBERS);
    out.push(FEATURE_APP_SELF);
    out.push(FEATURE_FILES_PATH_SCOPE);
    out.push(FEATURE_THREAD_ATTACHMENTS);
    out.push(FEATURE_THREAD_IMAGES);
    out.push(FEATURE_THREAD_DELTA);
    out.push(FEATURE_PROJECT_CHAT_PAGING);
    out.push(FEATURE_APP_PROJECT_CHAT);
    out.push(FEATURE_APP_MOBILE_ACCESS);
    out.push(FEATURE_APP_WALLET);
    out.push(FEATURE_ACTIVITY_LIVE_BUSY);
    if thread_push_ready() {
        out.push(FEATURE_THREAD_PUSH);
        // Same DB as 0157: migration 0189 (push_devices.previews) ran with it.
        out.push(FEATURE_PUSH_PREVIEWS);
        if push_e2e_ready() {
            out.push(FEATURE_PUSH_E2E);
        }
    }
    if thread_read_ready() {
        out.push(FEATURE_THREAD_READ);
    }
    if homes_sync_ready() {
        out.push(FEATURE_HOMES_SYNC);
    }
    if k2_core::db::try_shared().is_some() {
        out.push(FEATURE_INBOX_CATCHUP);
        out.push(FEATURE_CATCHUP_UNCHANGED);
    }
    if thread_read_ready() {
        out.push(FEATURE_APP_THREAD_PARITY);
        out.push(FEATURE_APP_READ_STATE);
        out.push(FEATURE_GARDEN_PERSON_ROWS);
        out.push(FEATURE_AGENT_UNREAD_TICKETS);
    }
    out.push(FEATURE_THREAD_STREAM);
    out.push(FEATURE_THREAD_ACTIVITY_EAR);
    out.push(FEATURE_THREAD_CONTEXT);
    out.push(FEATURE_APP_RESOURCES);
    out.push(FEATURE_LLM_ACCOUNT_FOR);
    out.push(FEATURE_COMPANION_PAIRING);
    // Permission cards (Beta, prd-agent-settings-cards-v1 §2.1, §10):
    // listed only while the global switch is on (setting or env). Its flip
    // publishes a settings frame (`settings_routes::permission_cards_flipped`)
    // so connected clients re-read this list.
    if k2_core::permission::gate::server_on() {
        out.push(k2_core::permission::FEATURE_KEY);
    }
    // Last on purpose: boot_status_views_integration pins it as the tail.
    out.push(FEATURE_BOOT_STATUS_VIEWS);
    out
}

/// PRD boot-status-ghost: `/boot-status` answers `view` (public | compat |
/// full) and `protocolMin`, reads a token (Bearer, `?token=`, cookie) for
/// the full body, and accepts `scope: "status"` keys. A client that sent a
/// token and sees this key with `view != "full"` knows its token was not
/// accepted (stale session), not that the daemon is old.
pub const FEATURE_BOOT_STATUS_VIEWS: &str = "boot-status-views-v1";

#[cfg(test)]
mod tests {
    use super::*;

    /// A widget manifest's `requires.features` names keys K2 knows
    /// (k2-core `KNOWN_FEATURES`); each zen one this daemon reports is in it.
    #[test]
    fn zen_feature_keys_are_known_to_the_widget_validator() {
        for f in features().into_iter().filter(|f| f.starts_with("zen-") || f.starts_with("thread-")) {
            assert!(k2_core::zen::widgets::KNOWN_FEATURES.contains(&f), "{f} missing from KNOWN_FEATURES");
        }
    }

    #[test]
    fn features_report_spawn_attach_only_and_tickets_list_all() {
        // The harness hook is never set in unit tests.
        assert!(attach_only_supported());
        // `thread-push-v1` is on only once the DB (with migration 0157) is
        // up: before init it is absent, after it present.
        let _ = k2_core::db::shared();
        assert_eq!(
            features(),
            vec![
                "spawn-attach-only",
                "tickets-list-all",
                "tickets-badge-me",
                "zen-v1",
                "zen-gardens-v1",
                "zen-chrome-v1",
                "zen-widgets-v1",
                "zen-lib-v1",
                "zen-sync-v1",
                "zen-garden-lock-v1",
                "thread-latest",
                "thread-widget-origin",
                "home-avatars-v1",
                "daemon-activity",
                "thread-address-stable",
                "sources-v1",
                "garden-data-v1",
                "grokbots-v1",
                "app-api-v1",
                "guests-rooms-v1",
                "room-members-v1",
                "app-self-v1",
                "files-path-scope-v1",
                "thread-attachments-v1",
                "thread-images-v1",
                "thread-delta-v1",
                "project-chat-paging-v1",
                "app-project-chat-v1",
                "app-mobile-access-v1",
                "app-wallet-v1",
                "activity-live-busy",
                "thread-push-v1",
                "push-previews-v1",
                "push-e2e-v1",
                "thread-read-v1",
                "homes-sync-v1",
                "inbox-catchup-v1",
                "catchup-unchanged-v1",
                "app-thread-parity-v1",
                "app-read-state-v1",
                "garden-person-rows-v1",
                "agent-unread-tickets-v1",
                "thread-stream-v1",
                "thread-activity-ear-v1",
                "thread-context-v1",
                "app-resources-v1",
                "llm-account-for-v1",
                "companion-pairing-v1",
                "boot-status-views-v1"
            ]
        );
    }

    // A private BootState, never the daemon's: the process-wide one is
    // shared with every in-process HTTP test (the test daemon harness sets
    // it ready, and the dispatcher 503s while it is not), so flipping it
    // here raced them in both directions.
    #[test]
    fn phase_transitions_and_detail_round_trip() {
        let boot = BootState::new();
        // Default before any boot call.
        assert_eq!(boot.phase_str(), "starting");
        assert!(!boot.is_ready());

        boot.set_migrating("Applying updates…");
        assert_eq!(boot.phase_str(), "migrating");
        assert!(!boot.is_ready());
        assert_eq!(boot.detail(), "Applying updates…");

        boot.set_detail("12/64 workspaces");
        assert_eq!(boot.detail(), "12/64 workspaces");
        assert!(!boot.is_ready(), "detail update must not flip readiness");

        boot.set_ready();
        assert_eq!(boot.phase_str(), "ready");
        assert!(boot.is_ready());
        assert_eq!(boot.detail(), "", "set_ready clears the detail");

        boot.set_error("migration 12 failed");
        assert_eq!(boot.phase_str(), "error");
        assert!(!boot.is_ready(), "an error is not ready");
        assert_eq!(boot.detail(), "migration 12 failed");
    }

    #[test]
    fn protocol_is_stable() {
        // Guards against an accidental bump — protocol changes are a
        // deliberate, breaking-contract decision.
        assert_eq!(PROTOCOL, 1);
        assert_eq!(PROTOCOL_MIN, 1);
        assert!(PROTOCOL_MIN <= PROTOCOL);
    }

    /// PRD boot-status-ghost §2.1: loopback is always full; elsewhere the
    /// owner token is full and no / an empty / a junk token gets the
    /// configured public view. (Sessions, passports and keys need the
    /// stores; `tests/boot_status_views_integration.rs` covers each.)
    #[test]
    fn view_for_table_without_stores() {
        use crate::routes::dispatcher::Ingress;
        use k2_core::app_settings::BootStatusPublic::{Compat, Ghost};
        const OWNER: &str = "owner-token-unit";
        for mode in [Compat, Ghost] {
            let stranger = if mode == Compat { View::Compat } else { View::Public };
            for tok in [None, Some(""), Some("junk"), Some("owner-token-unit-but-longer")] {
                assert_eq!(view_for(Ingress::Loopback, tok, OWNER, mode), View::Full, "loopback {tok:?}");
                for ing in [Ingress::Lan, Ingress::Tunnel, Ingress::App] {
                    assert_eq!(view_for(ing, tok, OWNER, mode), stranger, "{ing:?} {tok:?} {mode:?}");
                }
            }
            for ing in [Ingress::Lan, Ingress::Tunnel, Ingress::App] {
                assert_eq!(view_for(ing, Some(OWNER), OWNER, mode), View::Full, "{ing:?} owner");
                assert_eq!(view_for(ing, Some(" owner-token-unit "), OWNER, mode), View::Full, "trimmed");
            }
        }
        assert!(is_session_shaped(&"0a".repeat(32)));
        assert!(!is_session_shaped(&"0A".repeat(32)), "uppercase is not a session");
        assert!(!is_session_shaped(&"0a".repeat(31)));
    }

    /// The exact public bytes and the compat key order (§2.2), whatever
    /// serde_json's map features are.
    #[test]
    fn public_and_compat_bodies_have_the_exact_shape() {
        let s = k2_core::app_settings::AppSettings::default();
        let public = body_for(View::Public, &s);
        assert_eq!(
            public,
            format!(
                r#"{{"view":"public","phase":"{}","protocol":1,"protocolMin":1,"instanceId":"{}"}}"#,
                phase_str(),
                instance_id()
            )
        );
        let compat: serde_json::Value = serde_json::from_str(&body_for(View::Compat, &s)).unwrap();
        let raw = body_for(View::Compat, &s);
        let order: Vec<usize> = [
            "\"view\"", "\"phase\"", "\"protocol\"", "\"protocolMin\"", "\"instanceId\"", "\"version\"",
            "\"features\"", "\"serverId\"", "\"publicHost\"", "\"airgap\"",
        ]
        .iter()
        .map(|k| raw.find(k).unwrap_or_else(|| panic!("{k} missing from {raw}")))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "compat key order: {raw}");
        assert_eq!(compat.as_object().unwrap().len(), 10, "{raw}");
        assert_eq!(compat["airgap"].as_object().unwrap().len(), 1, "only airgap.enabled: {raw}");
        assert_eq!(compat["version"], env!("CARGO_PKG_VERSION"));

        let full: serde_json::Value = serde_json::from_str(&body_for(View::Full, &s)).unwrap();
        assert_eq!(full["view"], "full");
        assert_eq!(full["protocolMin"], PROTOCOL_MIN);
        for k in [
            "version", "protocol", "phase", "detail", "instanceId", "serverId", "publicHost", "features",
            "installKind", "webClient", "scopedHooks", "ops", "api", "airgap", "listen", "connectLogin",
        ] {
            assert!(full.get(k).is_some(), "full keeps {k}");
        }
        assert_eq!(full["webClient"]["enabled"], s.web_client_enabled);
        assert_eq!(full["connectLogin"]["ingress"], "edge");
    }

    #[test]
    fn ping_hides_the_version_only_from_ghost_strangers() {
        assert_eq!(ping_body(View::Public), "k2-daemon");
        assert!(!ping_body(View::Public).contains(env!("CARGO_PKG_VERSION")));
        assert_eq!(ping_body(View::Compat), crate::BANNER);
        assert_eq!(ping_body(View::Full), crate::BANNER);
    }

    #[test]
    fn instance_id_is_16_lowercase_hex_and_stable_within_process() {
        let first = instance_id();
        // Shape contract (0.40.48): exactly 16 lowercase hex chars.
        // Clients compare it opaquely, but the shape is asserted so a
        // future format drift is a deliberate decision, not an accident.
        assert_eq!(
            first.len(),
            16,
            "instance id must be exactly 16 chars; got {first:?}"
        );
        assert!(
            first.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "instance id must be lowercase hex only; got {first:?}"
        );
        // Stable for the life of the process — repeated reads return the
        // SAME id (same pointer, even: it's a OnceLock'd &'static str).
        let second = instance_id();
        assert_eq!(first, second, "instance id must not change within a process");
        assert!(
            std::ptr::eq(first, second),
            "instance id must be minted exactly once per process"
        );
    }

    #[test]
    fn classify_install_kind_detects_bundled_app() {
        use std::path::Path;
        assert_eq!(
            classify_install_kind(Some(Path::new(
                "/Applications/K2SO.app/Contents/MacOS/k2so-daemon"
            ))),
            "bundled-app"
        );
        // Bundle name doesn't matter — any `.app/Contents/` nesting counts.
        assert_eq!(
            classify_install_kind(Some(Path::new(
                "/Users/x/Build/K2 by Alakazam Labs.app/Contents/Resources/k2so-daemon"
            ))),
            "bundled-app"
        );
        // 0.43.2: the daemon runs from the nested helper app. Still
        // bundled-app, so updates stay on the app updater (Shape A).
        assert_eq!(
            classify_install_kind(Some(Path::new(
                "/Applications/K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon"
            ))),
            "bundled-app"
        );
    }

    #[test]
    fn classify_install_kind_detects_standalone() {
        use std::path::Path;
        assert_eq!(
            classify_install_kind(Some(Path::new("/usr/local/bin/k2so-daemon"))),
            "standalone"
        );
        assert_eq!(
            classify_install_kind(Some(Path::new("/home/u/.k2so/bin/k2so-daemon"))),
            "standalone"
        );
    }

    #[test]
    fn classify_install_kind_detects_package_installs() {
        use std::path::Path;
        for p in [
            "/usr/bin/k2-daemon",
            "/usr/lib/k2/k2-daemon",
            "/usr/sbin/k2-daemon",
            "/nix/store/abc123-k2-0.46.3/bin/k2-daemon",
        ] {
            assert_eq!(classify_install_kind(Some(Path::new(p))), "package", "{p}");
        }
        // Standalone installs stay standalone.
        for p in [
            "/usr/local/bin/k2-daemon",
            "/home/k2/.local/bin/k2-daemon",
            "/opt/k2/k2-daemon",
            "/usrx/bin/k2-daemon",
        ] {
            assert_eq!(classify_install_kind(Some(Path::new(p))), "standalone", "{p}");
        }
    }

    #[test]
    fn classify_install_kind_unknown_when_unresolved() {
        assert_eq!(classify_install_kind(None), "unknown");
    }
}
