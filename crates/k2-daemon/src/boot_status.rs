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
/// of `"bundled-app" | "standalone" | "unknown"`.
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
    "standalone"
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
/// Older pages of a project chat (project-chat-paging-v1, 0.46.2):
/// `GET /cli/project-group/messages?before_seq=N&limit=L` returns the newest
/// `L` messages with `seq < N` OLDEST first, `has_more` when older ones
/// remain (the `/cli/thread` `before_seq` page). A bad `before_seq` or
/// `limit`, or `before_seq` with `after_seq` / `after`, is 400 `usage`.
/// K2 Companion pages a project chat back only on a server with this key.
pub const FEATURE_PROJECT_CHAT_PAGING: &str = "project-chat-paging-v1";
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
    out.push(FEATURE_THREAD_DELTA);
    out.push(FEATURE_PROJECT_CHAT_PAGING);
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
    }
    out
}

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
                "thread-delta-v1",
                "project-chat-paging-v1",
                "thread-push-v1",
                "push-previews-v1",
                "push-e2e-v1",
                "thread-read-v1",
                "homes-sync-v1",
                "inbox-catchup-v1"
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
    fn classify_install_kind_unknown_when_unresolved() {
        assert_eq!(classify_install_kind(None), "unknown");
    }
}
