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
 *  stripping any `-pre`/`+build` suffix and coercing non-numerics to 0. */
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
