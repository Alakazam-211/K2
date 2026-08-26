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
