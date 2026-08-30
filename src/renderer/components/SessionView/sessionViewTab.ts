/** Per-window remembered Thread vs Terminal tab (C8). Not daemon-canonical. */

export type SessionViewTab = 'terminal' | 'thread' | 'chatter' | 'split'

export const SESSION_VIEW_TAB_DEFAULT: SessionViewTab = 'terminal'

const STORAGE_PREFIX = 'k2:session-view-tab:'

export function sessionViewTabStorageKey(hostKey: string, sessionKey: string): string {
  return `${STORAGE_PREFIX}${hostKey}:${sessionKey}`
}

export function parseSessionViewTab(raw: string | null | undefined): SessionViewTab {
  if (raw === 'thread' || raw === 'chatter' || raw === 'split') return raw
  return 'terminal'
}

/** Overlay UI is a viewer. Mount Thread/Chatter only while that tab is selected. */
export function overlayViewer(tab: SessionViewTab): {
  thread: boolean
  chatter: boolean
  hidePty: boolean
} {
  return {
    thread: tab === 'thread' || tab === 'split',
    chatter: tab === 'chatter',
    hidePty: tab === 'thread' || tab === 'chatter',
  }
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
