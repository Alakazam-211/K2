// Feature keys a daemon REPORTS in `/boot-status` `features` (Home M5).
//
// The version gates in `server-capabilities.ts` cannot tell main from the
// last release: both carry the same version string until the next cut. A
// behaviour a client must not guess at (e.g. whether `sessions/v2/spawn`
// honours `attach_only`) is therefore read from the list the daemon sends.
// `ServerScope.serverSupports` accepts these keys too (`kessel/server-scope`).
// Kept out of `server-capabilities.ts` so suites that mock that module still
// get the real registry.

/**
 * Feature keys the daemon REPORTS in `/boot-status` `features`, for what the
 * version string cannot tell apart (main and the last release share it).
 * A server that does not list the key — or reports no `features` at all,
 * like every daemon before the key was added — does not have it.
 */
export const REPORTED_FEATURES = {
  /** `sessions/v2/spawn` honours `attach_only`: a live session is returned
   *  as-is, anything else is `404 session_not_live`. Released daemons up to
   *  0.41.6 ignore the field and would SPAWN, so a view-only room on such a
   *  server never spawns a tab it has not seen live (`lib/room-spawn.ts`). */
  'spawn-attach-only': 'spawn-attach-only',
} as const

export type ReportedFeatureKey = keyof typeof REPORTED_FEATURES

export function isReportedFeature(feature: string): feature is ReportedFeatureKey {
  return Object.prototype.hasOwnProperty.call(REPORTED_FEATURES, feature)
}
