/** Subscription windows the daemon cached. No provider tokens. */

export interface UsageWindow {
  label: string
  /** 0–1 used. Percent left is `1 - used`. */
  used: number
  resetsAt: string
}

export interface HarnessUsage {
  harness: string
  plan: string
  windows: UsageWindow[]
  checkedAt: string
  /** "" when signed in with windows. Otherwise an empty-state sentence. */
  status: string
}

export interface SubscriptionDoc {
  harnesses: HarnessUsage[]
}

/** v1 probes. Anything else is absent — do not invent a row. */
export const PROBED_HARNESSES = ['claude', 'codex'] as const

const STALE_MS = 15_000

export function visibleHarnesses(doc: SubscriptionDoc): HarnessUsage[] {
  return doc.harnesses.filter(
    (h) => h.harness === 'claude' || h.harness === 'codex',
  )
}

export function harnessName(id: string): string {
  if (id === 'claude') return 'Claude'
  if (id === 'codex') return 'Codex'
  return id
}

/** Not signed in and sign-in expired do not count as a live window. */
export function isSignedIn(row: HarnessUsage): boolean {
  return row.status !== 'Not signed in' && row.status !== 'Sign-in expired'
}

export function percentLeft(used: number): number {
  const clamped = Math.min(1, Math.max(0, used))
  return Math.round((1 - clamped) * 100)
}

/**
 * Lowest remaining percent among signed-in probed windows, plus the
 * harness name. No signed-in window → "Usage".
 */
export function buttonLabel(doc: SubscriptionDoc | null): string {
  if (!doc) return 'Usage'
  let best: { name: string; left: number } | null = null
  for (const row of visibleHarnesses(doc)) {
    if (!isSignedIn(row)) continue
    for (const window of row.windows) {
      const left = percentLeft(window.used)
      if (best === null || left < best.left) {
        best = { name: harnessName(row.harness), left }
      }
    }
  }
  if (!best) return 'Usage'
  return `${best.name} ${best.left}%`
}

/** True when the menu should POST a refresh instead of only reading. */
export function isStale(doc: SubscriptionDoc | null, now: number): boolean {
  if (!doc) return true
  for (const name of PROBED_HARNESSES) {
    const row = doc.harnesses.find((h) => h.harness === name)
    if (!row) return true
    const checked = Date.parse(row.checkedAt)
    if (Number.isNaN(checked)) return true
    if (now - checked > STALE_MS) return true
  }
  return false
}

/** Time until `resetsAt`: `3h`, `4d`, `45m`. */
export function formatResetsIn(resetsAt: string, nowMs: number): string {
  const t = Date.parse(resetsAt)
  if (Number.isNaN(t)) return ''
  const delta = t - nowMs
  if (delta <= 0) return 'now'
  const minutes = Math.round(delta / 60_000)
  if (minutes < 90) return `${Math.max(1, minutes)}m`
  const hours = Math.round(delta / 3_600_000)
  if (hours < 48) return `${hours}h`
  const days = Math.round(delta / 86_400_000)
  return `${days}d`
}
