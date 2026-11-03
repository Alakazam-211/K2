/** Per-install: whether THIS Mac's app pings for updates on a timer.
 *  Thin-client localStorage — not daemon SSOT. Manual "Check for Updates"
 *  still runs. Default ON. */

export const LS_AUTO_UPDATE_CHECKS = 'k2.autoUpdateChecks'
export const AUTO_UPDATE_CHECKS_EVENT = 'k2-auto-update-checks'

/** Garbage / unset → enabled (today's behavior). Falsy strings disable. */
export function parseAutoUpdateChecks(raw: string | null): boolean {
  if (raw === null) return true
  switch (raw.trim().toLowerCase()) {
    case '0':
    case 'false':
    case 'off':
    case 'no':
      return false
    default:
      return true
  }
}

export function autoUpdateChecksEnabled(): boolean {
  try {
    if (typeof localStorage === 'undefined') return true
    return parseAutoUpdateChecks(localStorage.getItem(LS_AUTO_UPDATE_CHECKS))
  } catch {
    return true
  }
}

export function setAutoUpdateChecksEnabled(on: boolean): void {
  try {
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(LS_AUTO_UPDATE_CHECKS, on ? '1' : '0')
    }
  } catch {
    // Private mode — in-memory listeners still see the event this session.
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event(AUTO_UPDATE_CHECKS_EVENT))
  }
}

/** Per-install: when K2 last asked GitHub for an update and what it found,
 *  shared by every window. A reload or a second window inside the interval
 *  shows this instead of fetching again. Thin-client localStorage. */
export const LS_LAST_UPDATE_CHECK = 'k2.lastUpdateCheck'

/** An update a check found. `downloadUrl` and `canInstall` are the Linux
 *  package offer; both are empty for the in-app updater. */
export interface UpdateOffer {
  version: string
  notes: string | null
  downloadUrl: string | null
  canInstall: boolean
}

export interface LastUpdateCheck {
  at: number
  offer: UpdateOffer | null
}

const VERSION_SHAPE = /^v?\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]+)?$/
// The Download button opens this URL, so it must be K2's release page or asset.
const RELEASES_PREFIX = 'https://github.com/Alakazam-211/K2/releases/'

function isUpdateOffer(raw: unknown): raw is UpdateOffer {
  if (typeof raw !== 'object' || raw === null) return false
  const o = raw as Record<string, unknown>
  return (
    typeof o.version === 'string' &&
    VERSION_SHAPE.test(o.version) &&
    (o.notes === null || typeof o.notes === 'string') &&
    (o.downloadUrl === null ||
      (typeof o.downloadUrl === 'string' && o.downloadUrl.startsWith(RELEASES_PREFIX))) &&
    typeof o.canInstall === 'boolean'
  )
}

/** Garbage or a time in the future → null: nothing to trust, check as usual. */
export function parseLastUpdateCheck(raw: string | null, now: number): LastUpdateCheck | null {
  if (raw === null) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (typeof parsed !== 'object' || parsed === null) return null
  const { at, offer } = parsed as { at?: unknown; offer?: unknown }
  if (typeof at !== 'number' || !Number.isFinite(at) || at < 0 || at > now) return null
  if (offer !== null && !isUpdateOffer(offer)) return null
  return { at, offer }
}

export function readLastUpdateCheck(now: number = Date.now()): LastUpdateCheck | null {
  try {
    if (typeof localStorage === 'undefined') return null
    return parseLastUpdateCheck(localStorage.getItem(LS_LAST_UPDATE_CHECK), now)
  } catch {
    return null
  }
}

function writeLastUpdateCheck(check: LastUpdateCheck): void {
  try {
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(LS_LAST_UPDATE_CHECK, JSON.stringify(check))
    }
  } catch {
    // Private mode or full: checks run on this window's own timer instead.
  }
}

/** How long until an automatic check may fetch. 0 when it may fetch now,
 *  including when storage has no trustworthy time. */
export function msUntilUpdateCheckDue(intervalMs: number, now: number = Date.now()): number {
  const last = readLastUpdateCheck(now)
  if (!last) return 0
  return Math.max(0, last.at + intervalMs - now)
}

/** Called just before a fetch, so a reload loop can't refetch even if the
 *  fetch fails or the page dies. Keeps the update already found. */
export function rememberUpdateCheckStarted(now: number = Date.now()): void {
  writeLastUpdateCheck({ at: now, offer: readLastUpdateCheck(now)?.offer ?? null })
}

/** Called when a check gets an answer. A failed check doesn't call this, so
 *  the update found earlier stays on offer. An offer that wouldn't read back
 *  is dropped, keeping the time: a record that reads as garbage would refetch
 *  on every load. */
export function rememberUpdateCheckResult(offer: UpdateOffer | null, now: number = Date.now()): void {
  writeLastUpdateCheck({
    at: readLastUpdateCheck(now)?.at ?? now,
    offer: isUpdateOffer(offer) ? offer : null,
  })
}
