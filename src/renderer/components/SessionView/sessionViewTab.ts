/** Per-window remembered view (C8). Not daemon-canonical. */

export type SessionViewTab = 'terminal' | 'thread' | 'chatter' | 'split' | 'chat'

/** One side of Split view. Not `split` itself. */
export type SplitPaneView = 'chat' | 'terminal' | 'thread' | 'chatter'

export const SESSION_VIEW_TAB_DEFAULT: SessionViewTab = 'terminal'

/** Today's split: Terminal left, Thread right. A stored `split` with no side record uses this pair. */
export const DEFAULT_SPLIT_LEFT: SplitPaneView = 'terminal'
export const DEFAULT_SPLIT_RIGHT: SplitPaneView = 'thread'

export interface SessionSplitSides {
  left: SplitPaneView
  right: SplitPaneView
}

export const DEFAULT_SPLIT_SIDES: SessionSplitSides = {
  left: DEFAULT_SPLIT_LEFT,
  right: DEFAULT_SPLIT_RIGHT,
}

const STORAGE_PREFIX = 'k2:session-view-tab:'
const SPLIT_PREFIX = 'k2:session-view-split:'

export function sessionViewTabStorageKey(hostKey: string, sessionKey: string): string {
  return `${STORAGE_PREFIX}${hostKey}:${sessionKey}`
}

export function sessionViewSplitStorageKey(hostKey: string, sessionKey: string): string {
  return `${SPLIT_PREFIX}${hostKey}:${sessionKey}`
}

export function parseSplitPaneView(raw: unknown, fallback: SplitPaneView): SplitPaneView {
  if (raw === 'chat' || raw === 'terminal' || raw === 'thread' || raw === 'chatter') return raw
  return fallback
}

export function parseSessionViewTab(raw: string | null | undefined): SessionViewTab {
  if (raw === 'thread' || raw === 'chatter' || raw === 'split' || raw === 'chat') return raw
  return 'terminal'
}

/** Overlay UI is a viewer. Mount Thread/Chatter/Chat only while that view is selected.
 *  Split without stored sides is Terminal + Thread. */
export function overlayViewer(
  tab: SessionViewTab,
  sides?: { left?: SplitPaneView | null; right?: SplitPaneView | null } | null,
): {
  thread: boolean
  chatter: boolean
  chat: boolean
  hidePty: boolean
} {
  const views: SplitPaneView[] =
    tab === 'split'
      ? [
          parseSplitPaneView(sides?.left, DEFAULT_SPLIT_LEFT),
          parseSplitPaneView(sides?.right, DEFAULT_SPLIT_RIGHT),
        ]
      : tab === 'thread' || tab === 'chatter' || tab === 'chat'
        ? [tab]
        : ['terminal']
  return {
    thread: views.includes('thread'),
    chatter: views.includes('chatter'),
    chat: views.includes('chat'),
    hidePty: !views.includes('terminal'),
  }
}

export function parseSessionSplitSides(raw: string | null | undefined): SessionSplitSides {
  if (!raw) return DEFAULT_SPLIT_SIDES
  try {
    const parsed = JSON.parse(raw) as { left?: unknown; right?: unknown }
    if (!parsed || typeof parsed !== 'object') return DEFAULT_SPLIT_SIDES
    return {
      left: parseSplitPaneView(parsed.left, DEFAULT_SPLIT_LEFT),
      right: parseSplitPaneView(parsed.right, DEFAULT_SPLIT_RIGHT),
    }
  } catch {
    return DEFAULT_SPLIT_SIDES
  }
}

export function readSessionSplitSides(storageKey: string): SessionSplitSides {
  if (typeof localStorage === 'undefined') return DEFAULT_SPLIT_SIDES
  try {
    return parseSessionSplitSides(localStorage.getItem(storageKey))
  } catch {
    return DEFAULT_SPLIT_SIDES
  }
}

export function writeSessionSplitSides(storageKey: string, sides: SessionSplitSides): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(
      storageKey,
      JSON.stringify({ left: sides.left, right: sides.right }),
    )
  } catch {
    /* quota / private mode */
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
