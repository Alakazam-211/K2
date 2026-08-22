// Remote capability layer (#638) — makes reverse-compatibility EXPLICIT.
//
// The desktop client can target ANY daemon (local, self-hosted, hosted
// K2 Connect), and a remote may run an OLDER marketing version than the
// app. Newer client features (e.g. the RemoteFolderPicker's fs/info home
// seed, or the roles-gated Users/Access panel) degrade gracefully when a
// route 404s — but silently. This module adds KNOWLEDGE of the remote's
// version so callers can (a) gate cleanly and (b) tell the user exactly
// which host version unlocks a feature.
//
// The active host's version is cached in the connect-host store by the
// ConnectionGate when it accepts a host's /boot-status (#638). This module
// reads that cache and compares it against each feature's minimum version.
//
// Contract:
//   - activeHost === 'local'  → always supported (the local daemon is
//     byte-paired with this app; it speaks everything the app speaks).
//   - serverVersion unknown (null) → gated features return FALSE, but
//     callers MUST still tolerate it (the underlying route try/catch
//     fallback keeps working — this only drives hints/clean gating).
//   - serverVersion known → semver `gte(version, min)`.

import { useConnectHostStore, type ConnectHostState } from '@/stores/connect-host'

/**
 * Feature-key → minimum daemon (marketing) version that ships the route.
 * Bump/add entries as new remote-facing routes land. Keep keys stable —
 * components import them by literal.
 */
export const FEATURES = {
  /** GET /cli/fs/info — home dir + separator, so the RemoteFolderPicker
   *  can start at the user's home folder instead of '/'. */
  'fs-info': '0.39.24',
  /** GET /cli/auth/whoami role + /cli/users/* role management (#629). */
  roles: '0.39.23',
  /** POST /cli/daemon/restart — supervisor-agnostic remote daemon restart
   *  (#651/#661). Gates the "Restart host" control so an OLDER remote (no
   *  route) hides the button instead of showing a dead one that 404s. */
  'daemon-restart': '0.39.32',
  /** POST /cli/daemon/update/{check,start,apply} + GET
   *  /cli/daemon/update/status — host-aware remote self-update (P3/P4).
   *  Gates the "Update host" control so an OLDER remote (no routes) hides
   *  it instead of dead-ending on a 404. */
  'remote-update': '0.39.33',
  /** POST /cli/daemon/update/app — host-pushed desktop-app update (Shape A,
   *  0.39.35). Gates the future remote app-update path by remote daemon
   *  version; not yet wired to a control. */
  'remote-update-app': '0.39.35',
  /** Canonical daemon-owned Active set + daemon-side reaping (#672,
   *  .k2so/prds/daemon-canonical-active.md). The daemon owns the Active
   *  set and the grace-reap; it exposes GET /cli/projects/active, POST
   *  /cli/projects/{activate,pin,dismiss}, and pushes an `active_changed`
   *  event over the session-events WS. When SUPPORTED the renderer mirrors
   *  Active 1:1 from `useActiveStore` and routes gestures to those routes;
   *  when NOT, it falls back to the thin local Active-window derivation for
   *  DISPLAY only and does not call the new routes (an un-updated daemon
   *  simply won't reap — accepted transitional degradation, since the
   *  renderer reaper was deleted in this release). */
  'canonical-active': '0.39.38',
  /** POST /cli/workspace/ensure-pinned-chat — daemon-owned pinned-chat
   *  session lifecycle (#683, .k2so/prds/daemon-owned-pinned-chat.md).
   *  The daemon owns find-or-spawn + resume/fresh + refresh +
   *  dropdown-switch respawn; the renderer asks it to `ensure`, attaches
   *  the grid-WS, and renders. When SUPPORTED (always true for `local`)
   *  AgentChatPane uses the daemon-owned path (NO renderer resolve loop,
   *  NO self-retrigger guard / circuit breaker). When NOT (an older /
   *  remote daemon lacking the route) it falls back to the pre-0.39.39
   *  renderer-orchestrated path WITH the #682 band-aids intact. */
  'daemon-pinned-chat': '0.39.39',
  /** Daemon push broadcasts that retire renderer polling loops (#675/#677,
   *  Wave B spine). Over the existing `/cli/sessions/events` WS the daemon
   *  emits `llm_status_changed`, `agent_status_changed`,
   *  `tunnel_status_changed` (APP-LEVEL) and `review_queue_changed`,
   *  `review_changed` (WORKSPACE-SCOPED). When SUPPORTED (always true for
   *  `local`) the renderer drops its `setInterval` polls and subscribes;
   *  when NOT (an older / remote daemon that doesn't emit these frames) it
   *  KEEPS the existing polling fallback so status still updates. */
  'daemon-broadcasts': '0.39.39',
  /** `Role::Viewer` (presence/multiplayer S4): the daemon's
   *  /cli/users/set-role + add-user flow accept `"viewer"` and the
   *  presence roster carries viewer rows + POST /cli/presence/grant.
   *  Gates OFFERING the viewer option in the role dropdown so an older
   *  daemon (whose Role::from_wire rejects "viewer" with a 400) never
   *  gets a doomed set-role. */
  'viewer-role': '0.40.27',
  /** Daemon-side per-session activity detection (session_activity.rs):
   *  the daemon emits session_activity_changed (Title/Bell-derived
   *  working/idle/permission) for every session regardless of pane
   *  visibility. Gates the renderer subscription that feeds
   *  daemonPaneStatuses; an older/remote daemon never emits it, so the
   *  merge rule transparently falls back to client-side detection. */
  'session-activity': '0.40.39',
  /** Daemon-owned published services (`GET /cli/publish/list`,
   *  `POST /cli/publish/{start,stop}`, `publish_services_changed`).
   *  Gates Start/Stop in the workspace Published drawer. Older remotes
   *  hide the buttons; attributed nested URLs still render. Local is
   *  always true via serverSupports. */
  'publish-services': '0.40.106',
} as const

export type FeatureKey = keyof typeof FEATURES

/**
 * Semver `a >= b`, parsing `major.minor.patch` and IGNORING any
 * pre-release / build suffix (e.g. `0.39.24-rc.1` compares as `0.39.24`).
 * Missing components default to 0, so `0.39` is treated as `0.39.0`.
 */
export function gte(a: string, b: string): boolean {
  const pa = parseVersion(a)
  const pb = parseVersion(b)
  for (let i = 0; i < 3; i++) {
    if (pa[i] > pb[i]) return true
    if (pa[i] < pb[i]) return false
  }
  return true // equal
}

/** Parse `major.minor.patch` into a [number, number, number] tuple,
 *  stripping any `-pre`/`+build` suffix and coercing non-numerics to 0.
 *
 *  Used ONLY by `gte` for FEATURE GATING, where a prerelease build of the
 *  minimum version DOES carry the feature, so suffix-stripping is correct
 *  (`0.39.24-rc.1` gates as `0.39.24`). For ordering two product versions
 *  (the update-display path) use {@link compareVersions}, which treats a
 *  prerelease as sorting BEFORE its release. */
function parseVersion(v: string): [number, number, number] {
  // Drop a leading 'v' and any pre-release/build suffix.
  const core = v.trim().replace(/^v/i, '').split(/[-+]/)[0] ?? ''
  const parts = core.split('.')
  const num = (s: string | undefined): number => {
    const n = Number.parseInt(s ?? '', 10)
    return Number.isFinite(n) ? n : 0
  }
  return [num(parts[0]), num(parts[1]), num(parts[2])]
}

/**
 * Order two version strings — the renderer twin of the daemon's
 * `compare_versions` (`crates/k2-daemon/src/update_routes.rs`). KEEP THE
 * TWO IN LOCKSTEP. THE ONE CANONICAL RULE:
 *
 *   1. A leading `v`/`V` is dropped.
 *   2. Build metadata (`+...`) is ignored.
 *   3. The release core (`major.minor.patch[.…]`) compares numerically,
 *      longest-wins on a prefix tie (`1.2.0` < `1.2.0.1`); non-numeric core
 *      components count as 0.
 *   4. A version WITH a prerelease suffix (`-rc1`) sorts BEFORE the same
 *      release WITHOUT one — `0.40.0-rc1 < 0.40.0 < 0.40.1`. Two prereleases
 *      of the same release compare by their identifier ASCII-lexically
 *      (`-rc1 < -rc2`).
 *
 * Returns -1 if `a < b`, 0 if equal, 1 if `a > b`.
 *
 * NOTE: this is the version-ORDERING rule (used for displaying / comparing
 * product versions). Feature gating uses {@link gte}, which deliberately
 * strips prerelease suffixes (an rc of the min version has the feature).
 */
export function compareVersions(a: string, b: string): -1 | 0 | 1 {
  const split = (v: string): { core: number[]; pre: string | null } => {
    const noV = v.trim().replace(/^v/i, '')
    const noBuild = noV.split('+')[0] ?? '' // drop build metadata
    const dash = noBuild.indexOf('-')
    const coreStr = dash === -1 ? noBuild : noBuild.slice(0, dash)
    const pre = dash === -1 ? null : noBuild.slice(dash + 1)
    const core = coreStr.split('.').map((c) => {
      const n = Number.parseInt(c, 10)
      return Number.isFinite(n) ? n : 0
    })
    return { core, pre }
  }
  const pa = split(a)
  const pb = split(b)
  const n = Math.max(pa.core.length, pb.core.length)
  for (let i = 0; i < n; i++) {
    const x = pa.core[i] ?? 0
    const y = pb.core[i] ?? 0
    if (x < y) return -1
    if (x > y) return 1
  }
  // Equal release core → a prerelease sorts BEFORE the bare release.
  if (pa.pre === null && pb.pre === null) return 0
  if (pa.pre !== null && pb.pre === null) return -1
  if (pa.pre === null && pb.pre !== null) return 1
  // both prereleases → ASCII-lexical on the identifier
  if (pa.pre! < pb.pre!) return -1
  if (pa.pre! > pb.pre!) return 1
  return 0
}

/** The minimum daemon version that supports `feature` — for hint copy. */
export function featureMinVersion(feature: FeatureKey): string {
  return FEATURES[feature]
}

/**
 * Does the ACTIVE host support `feature`?
 *   - local → always true.
 *   - remote with unknown version → false (caller must still tolerate it).
 *   - remote with a known version → semver gte against the feature min.
 *
 * Reads the connect-host store at call time (non-reactive). Components that
 * need to re-render on a host/version change use {@link useServerSupports}.
 */
export function serverSupports(feature: FeatureKey): boolean {
  const state = useConnectHostStore.getState()
  return supportsFor(state, feature)
}

/** Pure predicate over a store snapshot — shared by the function + hook so
 *  both apply identical rules (and so it's trivially unit-testable). */
function supportsFor(
  state: Pick<ConnectHostState, 'activeHost' | 'serverVersion'>,
  feature: FeatureKey,
): boolean {
  if (state.activeHost === 'local') return true
  const version = state.serverVersion
  if (!version) return false
  return gte(version, FEATURES[feature])
}

/**
 * React hook form of {@link serverSupports} — subscribes to the active host
 * + its cached version, so a component re-renders when either changes.
 */
export function useServerSupports(feature: FeatureKey): boolean {
  return useConnectHostStore((s) => supportsFor(s, feature))
}
