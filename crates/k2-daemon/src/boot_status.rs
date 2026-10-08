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
use std::sync::{LazyLock, OnceLock, RwLock};

/// daemon↔client API compatibility version. Bump this ONLY when a
/// change breaks the routes/contract clients depend on — NOT on every
/// release. K2 Connect range-checks `protocol` to decide whether it can
/// talk to a remote daemon of a different marketing version; the local
/// auto-update path keys off the exact `version` string instead. The
/// two are intentionally decoupled. Starts at 1.
pub const PROTOCOL: u32 = 1;

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

static PHASE: AtomicU8 = AtomicU8::new(STARTING);
static DETAIL: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new(String::new()));

/// Enter the `migrating` phase with an initial human-readable detail.
pub fn set_migrating(detail: &str) {
    set_detail(detail);
    PHASE.store(MIGRATING, Ordering::SeqCst);
}

/// Update the human-readable detail string. UI-only — clients MUST NOT
/// parse this for logic; it exists purely so the window can show
/// "Applying updates… (12/64 workspaces)" while we work.
pub fn set_detail(detail: &str) {
    if let Ok(mut d) = DETAIL.write() {
        *d = detail.to_string();
    }
}

/// Mark the daemon fully booted: migrations done, every provider/sink
/// registered, real routes safe to serve. Clears the detail. This is
/// the gate the dispatcher and the renderer both wait on.
pub fn set_ready() {
    set_detail("");
    PHASE.store(READY, Ordering::SeqCst);
}

/// Mark a fatal boot/migration error with a human-readable detail.
/// Reserved for future wiring (e.g. a migration that hard-fails) so the
/// renderer can surface a real error instead of spinning forever.
#[allow(dead_code)]
pub fn set_error(detail: &str) {
    set_detail(detail);
    PHASE.store(ERROR, Ordering::SeqCst);
}

/// True once [`set_ready`] has run. The dispatcher uses this to 503
/// every non-liveness route until first-boot migrations complete.
pub fn is_ready() -> bool {
    PHASE.load(Ordering::SeqCst) == READY
}

/// Lowercase phase string for the `/boot-status` JSON. Unknown future
/// values never appear here, but clients should treat any phase other
/// than `ready` as "not ready" (forward-compatible).
pub fn phase_str() -> &'static str {
    match PHASE.load(Ordering::SeqCst) {
        MIGRATING => "migrating",
        READY => "ready",
        ERROR => "error",
        _ => "starting",
    }
}

/// Current human-readable detail (cloned). UI-only.
pub fn detail() -> String {
    match DETAIL.read() {
        Ok(d) => d.clone(),
        // Poisoned lock should never happen (no panics under the lock),
        // but degrade to an empty detail rather than taking the daemon
        // down over a status string.
        Err(_) => String::new(),
    }
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

/// Client-visible features this daemon has that its version string cannot
/// tell apart (`/boot-status` `features`). A client treats a key that is
/// absent — or a daemon with no `features` at all — as unsupported.
pub fn features() -> Vec<&'static str> {
    let mut out = Vec::new();
    if attach_only_supported() {
        out.push(FEATURE_SPAWN_ATTACH_ONLY);
    }
    out.push(FEATURE_TICKETS_LIST_ALL);
    out.push(FEATURE_ZEN_V1);
    out.push(FEATURE_ZEN_GARDENS);
    out.push(FEATURE_ZEN_CHROME);
    out.push(FEATURE_ZEN_WIDGETS);
    out.push(FEATURE_ZEN_LIB);
    out.push(FEATURE_ZEN_SYNC);
    out.push(FEATURE_THREAD_LATEST);
    out.push(FEATURE_THREAD_WIDGET_ORIGIN);
    out.push(FEATURE_HOME_AVATARS);
    out.push(FEATURE_DAEMON_ACTIVITY);
    out.push(FEATURE_THREAD_ADDRESS_STABLE);
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
        assert_eq!(
            features(),
            vec![
                "spawn-attach-only",
                "tickets-list-all",
                "zen-v1",
                "zen-gardens-v1",
                "zen-chrome-v1",
                "zen-widgets-v1",
                "zen-lib-v1",
                "zen-sync-v1",
                "thread-latest",
                "thread-widget-origin",
                "home-avatars-v1",
                "daemon-activity",
                "thread-address-stable"
            ]
        );
    }

    // These mutate process-global state, so they live in ONE test to
    // run serially and not race the shared AtomicU8 / RwLock across
    // cargo's parallel test threads.
    #[test]
    fn phase_transitions_and_detail_round_trip() {
        // Default before any boot call.
        assert_eq!(phase_str(), "starting");
        assert!(!is_ready());

        set_migrating("Applying updates…");
        assert_eq!(phase_str(), "migrating");
        assert!(!is_ready());
        assert_eq!(detail(), "Applying updates…");

        set_detail("12/64 workspaces");
        assert_eq!(detail(), "12/64 workspaces");
        assert!(!is_ready(), "detail update must not flip readiness");

        set_ready();
        assert_eq!(phase_str(), "ready");
        assert!(is_ready());
        assert_eq!(detail(), "", "set_ready clears the detail");
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
