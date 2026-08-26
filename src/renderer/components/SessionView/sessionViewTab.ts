/** Per-window remembered Thread vs Terminal tab (C8). Not daemon-canonical. */

export type SessionViewTab = 'thread' | 'terminal'

export const SESSION_VIEW_TAB_DEFAULT: SessionViewTab = 'terminal'

const STORAGE_PREFIX = 'k2:session-view-tab:'

export function sessionViewTabStorageKey(hostKey: string, sessionKey: string): string {
  return `${STORAGE_PREFIX}${hostKey}:${sessionKey}`
}

export function parseSessionViewTab(raw: string | null | undefined): SessionViewTab {
  return raw === 'thread' ? 'thread' : 'terminal'
}

export function readSessionViewTab(storageKey: string): SessionViewTab {
  if (typeof localStorage === 'undefined') return SESSION_VIEW_TAB_DEFAULT
  try {
    return parseSessionViewTab(localStorage.getItem(storageKey))
  } catch {
    return SESSION_VIEW_TAB_DEFAULT
  }
}

export function writeSessionViewTab(storageKey: string, tab: SessionViewTab): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(storageKey, tab)
  } catch {
    /* quota / private mode */
  }
}
