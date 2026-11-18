// "Get this server ready for Apps" (0.46.16): typed bindings for the owner's
// card. The daemon decides when to offer it, who sees it (owner logins only),
// runs the setup through the root helper and schedules the restart; this file
// reads that truth and sends the owner's clicks to the window's server.
//
//   GET  /cli/app/box/ready-offer                 the card ({owner:false} for others)
//   POST /cli/app/box/ready-offer/trigger {trigger:"apps_page"}
//   POST /cli/app/box/ready-offer/dismiss {trigger}   "Not now"
//   POST /cli/app/box/ready-offer/run {}          the setup (never automatic)
//   POST /cli/app/box/ready-offer/restart {when:"now"|"tonight"|"cancel", tz?}

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

/** Opens the card from Settings → Cores → Apps → This server. */
export const OPEN_EVENT = 'k2:apps-ready-open'
/** The Apps page opened (Settings → Cores → Apps): the card tells the daemon. */
export const APPS_PAGE_EVENT = 'k2:apps-page-opened'

export interface ReadyOffer {
  trigger: string
  title: string
  line: string
}

export interface BlockingRow {
  id: string
  status: string
  detail: string
  fix: string | null
}

export interface ReadyRun {
  /** running | done | failed */
  state: string
  /** prepare | engine | check | finished */
  phase: string
  error: string | null
  ready: boolean | null
  blocking: BlockingRow[]
  restartNeeded: boolean
  changed: string[]
}

export interface ScheduledRestart {
  at: number
  zone: string
}

export interface Guests {
  connected: number
  signedIn: number
}

export interface ReadyStatus {
  owner: boolean
  linux: boolean
  ready: boolean
  offer: ReadyOffer | null
  run: ReadyRun | null
  restartNeeded: boolean
  restart: ScheduledRestart | null
  guests: Guests | null
  blocking: BlockingRow[]
}

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)
const str = (v: unknown): string => (typeof v === 'string' ? v : '')
const num = (v: unknown): number => (typeof v === 'number' ? v : 0)

function rows(v: unknown): BlockingRow[] {
  return Array.isArray(v)
    ? v.filter(isObj).map((r) => ({ id: str(r.id), status: str(r.status), detail: str(r.detail), fix: typeof r.fix === 'string' ? r.fix : null }))
    : []
}

export function parseStatus(raw: unknown): ReadyStatus {
  const v = isObj(raw) ? raw : {}
  const o = isObj(v.offer) ? v.offer : null
  const r = isObj(v.run) ? v.run : null
  const rs = isObj(v.restart) ? v.restart : null
  const g = isObj(v.guests) ? v.guests : null
  return {
    owner: v.owner === true,
    linux: v.linux === true,
    ready: v.ready === true,
    offer: o ? { trigger: str(o.trigger), title: str(o.title), line: str(o.line) } : null,
    run: r
      ? {
          state: str(r.state),
          phase: str(r.phase),
          error: typeof r.error === 'string' ? r.error : null,
          ready: typeof r.ready === 'boolean' ? r.ready : null,
          blocking: rows(r.blocking),
          restartNeeded: r.restartNeeded === true,
          changed: Array.isArray(r.changed) ? r.changed.filter((c): c is string => typeof c === 'string') : [],
        }
      : null,
    restartNeeded: v.restartNeeded === true,
    restart: rs ? { at: num(rs.at), zone: str(rs.zone) } : null,
    guests: g ? { connected: num(g.connected), signedIn: num(g.signedIn) } : null,
    blocking: rows(v.blocking),
  }
}

export async function fetchReady(): Promise<ReadyStatus> {
  return parseStatus(await daemonCliGet(primaryScope(), 'app/box/ready-offer'))
}

/** The owner opened the Apps page: the daemon records it once (owners
 *  only; anyone else records nothing) and answers with the card. */
export async function noteAppsPageOpened(): Promise<ReadyStatus> {
  return parseStatus(await daemonCliPost(primaryScope(), 'app/box/ready-offer/trigger', { trigger: 'apps_page' }))
}

export async function dismissOffer(trigger: string): Promise<void> {
  await daemonCliPost(primaryScope(), 'app/box/ready-offer/dismiss', { trigger })
}

export async function startRun(): Promise<ReadyStatus> {
  return parseStatus(await daemonCliPost(primaryScope(), 'app/box/ready-offer/run', {}))
}

/** This computer's IANA zone ("Tonight at 3 am" is the owner's 3 am). */
export function localZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null
  } catch {
    return null
  }
}

export async function restartTonight(): Promise<ReadyStatus> {
  const tz = localZone()
  return parseStatus(await daemonCliPost(primaryScope(), 'app/box/ready-offer/restart', tz ? { when: 'tonight', tz } : { when: 'tonight' }))
}

export async function restartNow(): Promise<void> {
  await daemonCliPost(primaryScope(), 'app/box/ready-offer/restart', { when: 'now' })
}

export async function cancelRestart(): Promise<ReadyStatus> {
  return parseStatus(await daemonCliPost(primaryScope(), 'app/box/ready-offer/restart', { when: 'cancel' }))
}

/** What the run is doing, in words. */
export function phaseText(phase: string): string {
  switch (phase) {
    case 'engine':
      return 'Installing the App engine…'
    case 'check':
      return 'Checking the server…'
    default:
      return 'Installing Podman and system packages, setting up the App user and firewall…'
  }
}

/** What a restart would do to guests right now. */
export function guestsText(g: Guests | null): string {
  if (!g) return 'A restart signs guests out of older App sites.'
  if (g.connected === 0) return 'Nobody is connected to your Apps right now, so a restart signs nobody out.'
  return `${g.connected} ${g.connected === 1 ? 'guest is' : 'guests are'} connected to your Apps right now and would be signed out of older App sites.`
}

/** `Nov 19, 2026, 3:00 AM` in this computer's time. */
export function whenText(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}
