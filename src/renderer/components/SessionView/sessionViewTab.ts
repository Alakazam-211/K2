import { hostScopedKey } from '@/lib/host-scoped-storage'
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

/** `hostKey` is the server's host key (`lib/host-key.ts`); the stored key
 *  is `<hostKey>|k2:session-view-tab:<sessionKey>` (Home M1, MS6). */
export function sessionViewTabStorageKey(hostKey: string, sessionKey: string): string {
  return hostScopedKey(hostKey, `${STORAGE_PREFIX}${sessionKey}`)
}

export function sessionViewSplitStorageKey(hostKey: string, sessionKey: string): string {
  return hostScopedKey(hostKey, `${SPLIT_PREFIX}${sessionKey}`)
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

/** Replacement for a Chat side when the harness has no chat face: Terminal,
 *  or Thread if the other side is already Terminal. */
export function chatFallbackSide(other: SplitPaneView): SplitPaneView {
  return other === 'terminal' ? 'thread' : 'terminal'
}

// ── "New agents open in" (Settings → General → Workspaces) ─────────────
// The view a conversation opens in when this client has no remembered view
// for it yet. Per-install localStorage, like `k2.workspaceSwitchFocus` and
// the per-session view memory above: it is how THIS viewer likes to look at
// agents, not canonical server state (the daemon's settings.json is shared
// by every user of the server, so it is the wrong home).

export const LS_NEW_AGENT_VIEW = 'k2.newAgentView'

export interface NewAgentViewPref {
  view: SessionViewTab
  /** Sides used when `view` is `split` (and the first time the user picks
   *  Split on that conversation). Always two different views. */
  split: SessionSplitSides
}

export const NEW_AGENT_VIEW_DEFAULT: NewAgentViewPref = {
  view: SESSION_VIEW_TAB_DEFAULT,
  split: DEFAULT_SPLIT_SIDES,
}

const SPLIT_PANE_ORDER: SplitPaneView[] = ['terminal', 'thread', 'chat', 'chatter']

/** Coerce anything to a legal pref. Unknown view → Terminal; missing sides →
 *  Terminal | Thread; equal sides → the right side moves to the next view. */
export function normalizeNewAgentView(raw: unknown): NewAgentViewPref {
  if (!raw || typeof raw !== 'object') return NEW_AGENT_VIEW_DEFAULT
  // Accepts the pref shape (`split: {left, right}`) and the stored shape
  // (flat `left` / `right`).
  const obj = raw as {
    view?: unknown
    left?: unknown
    right?: unknown
    split?: { left?: unknown; right?: unknown } | null
  }
  const view = parseSessionViewTab(typeof obj.view === 'string' ? obj.view : null)
  const sides = obj.split && typeof obj.split === 'object' ? obj.split : obj
  const left = parseSplitPaneView(sides.left, DEFAULT_SPLIT_LEFT)
  let right = parseSplitPaneView(sides.right, DEFAULT_SPLIT_RIGHT)
  if (right === left) {
    right = SPLIT_PANE_ORDER.find((v) => v !== left) ?? DEFAULT_SPLIT_RIGHT
  }
  return { view, split: { left, right } }
}

export function readNewAgentView(): NewAgentViewPref {
  if (typeof localStorage === 'undefined') return NEW_AGENT_VIEW_DEFAULT
  try {
    const raw = localStorage.getItem(LS_NEW_AGENT_VIEW)
    if (!raw) return NEW_AGENT_VIEW_DEFAULT
    return normalizeNewAgentView(JSON.parse(raw))
  } catch {
    return NEW_AGENT_VIEW_DEFAULT
  }
}

export function writeNewAgentView(pref: NewAgentViewPref): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(
      LS_NEW_AGENT_VIEW,
      JSON.stringify({ view: pref.view, left: pref.split.left, right: pref.split.right }),
    )
  } catch {
    /* quota / private mode */
  }
}

/** The view for one conversation on this client. A conversation with a
 *  remembered view keeps it (a stored `split` with no side record keeps
 *  Terminal | Thread). One with no remembered view is new here: it gets
 *  the "New agents open in" choice and `fresh` is true so the caller can
 *  remember it, which keeps later changes to the setting from moving it. */
export function resolveSessionView(
  storageKey: string | null,
  splitKey: string | null,
): { tab: SessionViewTab; sides: SessionSplitSides; fresh: boolean } {
  let storedTab: string | null = null
  let storedSplit: string | null = null
  if (storageKey && splitKey && typeof localStorage !== 'undefined') {
    try {
      storedTab = localStorage.getItem(storageKey)
      storedSplit = localStorage.getItem(splitKey)
    } catch {
      /* blocked storage — treat as new */
    }
  }
  if (storedTab !== null) {
    return {
      tab: parseSessionViewTab(storedTab),
      sides: parseSessionSplitSides(storedSplit),
      fresh: false,
    }
  }
  const pref = readNewAgentView()
  return {
    tab: pref.view,
    sides: storedSplit !== null ? parseSessionSplitSides(storedSplit) : pref.split,
    fresh: storageKey !== null,
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
