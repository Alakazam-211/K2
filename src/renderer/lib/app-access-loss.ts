// App-governed Spaces S7: what uninstalling an App would cost its people,
// read from the daemon's refusals (the daemon decides; this only words it).
//
// - `409 uninstall_drops_access` from `POST /cli/app/uninstall {app}`:
//   `error.wouldDrop` = that App's counts.
// - the same code from `POST /cli/app/release {apps: "uninstall"}`:
//   `error.apps` = [{app, wouldDrop}] for every App that would lose
//   something; nothing was released.
// - `409 workspace_has_apps` from a workspace removal: `error.apps` =
//   [{app, state}], the Apps the workspace still maintains.

import { httpCodeOf, httpDetailsOf } from './daemon-http-error'

/** The refusal's `error` object: `DaemonHttpError.details`, else the
 *  `{"error":{…}}` body some callers throw as the message. */
function refusalOf(err: unknown): Record<string, unknown> | null {
  const d = httpDetailsOf(err)
  if (d) return d
  const msg = err instanceof Error ? err.message : typeof err === 'string' ? err : ''
  const start = msg.indexOf('{')
  if (start < 0) return null
  try {
    const parsed = JSON.parse(msg.slice(start)) as { error?: unknown } | null
    const e = parsed?.error
    return e !== null && typeof e === 'object' && !Array.isArray(e) ? (e as Record<string, unknown>) : null
  } catch {
    return null
  }
}

function codeOf(err: unknown): string | null {
  const c = httpCodeOf(err)
  if (c) return c
  const r = refusalOf(err)
  return r && typeof r.code === 'string' ? r.code : null
}

export const UNINSTALL_DROPS_ACCESS = 'uninstall_drops_access'
export const WORKSPACE_HAS_APPS = 'workspace_has_apps'

export interface AppLoss {
  app: string
  /** "2 App logins", "1 chat Space bound to it", … (counts above zero). */
  parts: string[]
}

const WORDS: Array<[string, string, string]> = [
  ['logins', 'App login', 'App logins'],
  ['chatGrants', 'chat grant the App gave', 'chat grants the App gave'],
  ['otherGrants', 'other grant on the App', 'other grants on the App'],
  ['ownerGrants', 'owner grant to the App', 'owner grants to the App'],
  ['secretShares', 'secret shared with it', 'secrets shared with it'],
  ['spaces', 'chat Space bound to it', 'chat Spaces bound to it'],
]

/** The daemon's counts in words (only those above zero). */
export function lossParts(wouldDrop: unknown): string[] {
  if (wouldDrop === null || typeof wouldDrop !== 'object') return []
  const c = wouldDrop as Record<string, unknown>
  const out: string[] = []
  for (const [key, one, many] of WORDS) {
    const n = c[key]
    if (typeof n !== 'number' || !Number.isFinite(n) || n <= 0) continue
    out.push(n === 1 ? `1 ${one}` : `${n} ${many}`)
  }
  return out
}

/** The Apps an uninstall (or a release) would cost, from its refusal; null
 *  for any other error. `app` names the App of a single uninstall. */
export function accessLossOf(err: unknown, app?: string): AppLoss[] | null {
  if (codeOf(err) !== UNINSTALL_DROPS_ACCESS) return null
  const d = refusalOf(err)
  if (!d) return null
  if (Array.isArray(d.apps)) {
    const out: AppLoss[] = []
    for (const row of d.apps) {
      if (row === null || typeof row !== 'object') continue
      const r = row as Record<string, unknown>
      if (typeof r.app !== 'string' || r.app === '') continue
      out.push({ app: r.app, parts: lossParts(r.wouldDrop) })
    }
    return out.length > 0 ? out : null
  }
  if (d.wouldDrop && app) return [{ app, parts: lossParts(d.wouldDrop) }]
  return null
}

/** The Apps a workspace still maintains, from a removal's
 *  `workspace_has_apps` refusal; null for any other error. */
export function appsBlockingRemovalOf(err: unknown): string[] | null {
  if (codeOf(err) !== WORKSPACE_HAS_APPS) return null
  const d = refusalOf(err)
  const apps = d && Array.isArray(d.apps) ? d.apps : []
  const names = apps
    .map((a) => (a !== null && typeof a === 'object' ? (a as Record<string, unknown>).app : null))
    .filter((a): a is string => typeof a === 'string' && a !== '')
  return names
}
