// The Workspace drawer's Apps section: the Apps this workspace's agent
// maintains, read from `GET /cli/app/list?workspace=<project id>` (the
// daemon filters on the App's `owner_workspace`; the renderer never
// filters). Pure parsing and wording only; AppsSection.tsx renders.
//
// Each `apps[]` row (k2-daemon apps/routes.rs `app_summary`) carries:
//   appId     the App's name (there is no separate display name)
//   serviceId its runtime row in Published (`published_services.id`, the
//             `id` of a GET /cli/publish/list service). Made with the App
//             and moved with it (hand over, park), so it holds in every state
//   label     the run path in words ("Guarded (container)"), optional
//   state     installing | running | stopped | crashlooping | failed |
//             parked | waiting_engine | restarting
//   stateReason  why, when the daemon has one (the state's tooltip)
//   url       the App's k2 address, or null
//   hosts     always present: the url's host, then the custom domains K2
//             serves for it ([] when none; then the url, if any, is shown)

/** The command the empty state names (Published's voice). */
import { CLI_NAME } from '@/lib/brand'

export const APP_DEPLOY_EXAMPLE = `${CLI_NAME} app deploy`

export interface WorkspaceApp {
  appId: string
  serviceId: string | null
  label: string | null
  state: string
  stateReason: string | null
  url: string | null
  hosts: string[]
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

/** `hosts`: strings only; anything else reads as none. */
export function parseHosts(raw: unknown): string[] {
  if (!Array.isArray(raw)) return []
  return raw.filter((h): h is string => typeof h === 'string' && h.trim() !== '').map((h) => h.trim())
}

/** `GET /cli/app/list` → its Apps, in the daemon's order. */
export function parseWorkspaceApps(raw: unknown): WorkspaceApp[] {
  const apps = raw !== null && typeof raw === 'object' ? (raw as { apps?: unknown }).apps : null
  if (!Array.isArray(apps)) return []
  const out: WorkspaceApp[] = []
  for (const a of apps) {
    if (a === null || typeof a !== 'object') continue
    const o = a as Record<string, unknown>
    const appId = str(o.appId)
    if (!appId) continue
    out.push({
      appId,
      serviceId: str(o.serviceId),
      label: str(o.label),
      state: str(o.state) ?? '',
      stateReason: str(o.stateReason),
      url: str(o.url),
      hosts: parseHosts(o.hosts),
    })
  }
  return out
}

/** The Published service ids that belong to this workspace's Apps (from
 *  the same GET /cli/app/list answer). Published leaves those rows out:
 *  they live in Apps, and an App's Details still opens their sheet. */
export function appServiceIds(raw: unknown): Set<string> {
  const ids = new Set<string>()
  for (const app of parseWorkspaceApps(raw)) if (app.serviceId) ids.add(app.serviceId)
  return ids
}

function hostOf(url: string): string {
  try {
    return new URL(url).host || url
  } catch {
    return url.replace(/^[a-z]+:\/\//i, '').replace(/\/.*$/, '')
  }
}

/** What the row shows for the App's addresses: the first (the k2 host),
 *  how many more, the full list for the tooltip, and what Open opens. */
export function appAddresses(app: WorkspaceApp): {
  first: string | null
  more: number
  all: string[]
  openUrl: string | null
} {
  const all = app.hosts.length > 0 ? app.hosts : app.url ? [hostOf(app.url)] : []
  const first = all[0] ?? null
  const openUrl = app.url ?? (first ? `https://${first}` : null)
  return { first, more: Math.max(0, all.length - 1), all, openUrl }
}

/** ok: running. working: on its way (the in-progress pulse). danger:
 *  broken. quiet: not serving, on purpose or waiting. */
export type AppStateTone = 'ok' | 'working' | 'danger' | 'quiet'

/** An App's state as one plain word (or three) and its tone. Never
 *  colour alone: the row always prints the word beside the dot. */
export function appStateView(state: string): { tone: AppStateTone; word: string } {
  switch (state) {
    case 'running':
      return { tone: 'ok', word: 'Running' }
    case 'installing':
      return { tone: 'working', word: 'Installing' }
    case 'restarting':
      return { tone: 'working', word: 'Restarting' }
    case 'failed':
      return { tone: 'danger', word: 'Failed' }
    case 'crashlooping':
      return { tone: 'danger', word: 'Crashing' }
    case 'stopped':
      return { tone: 'quiet', word: 'Stopped' }
    case 'parked':
      return { tone: 'quiet', word: 'Parked' }
    case 'waiting_engine':
      return { tone: 'quiet', word: 'Waiting for engine' }
    default:
      return {
        tone: 'quiet',
        word: state ? state.charAt(0).toUpperCase() + state.slice(1).replace(/_/g, ' ') : 'Unknown',
      }
  }
}

/** The list GET's failure in words (Published's shape). */
export function appListErrorMessage(err: unknown): string {
  if (err instanceof Error && err.message.trim()) return err.message.trim()
  if (typeof err === 'string' && err.trim()) return err.trim()
  return 'Failed to load Apps'
}
