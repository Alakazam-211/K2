import { hostScopedKey } from '@/lib/host-scoped-storage'
/** Per-window remembered view (C8). Not daemon-canonical. */

export type SessionViewTab = 'terminal' | 'thread' | 'chatter' | 'split'

/** One side of Split view. Not `split` itself. */
export type SplitPaneView = 'terminal' | 'thread' | 'chatter'

// The Chat view (the agent's JSONL transcript drawn over its PTY, LIVE
// 0.41.0) was retired from the UI in 0.46.4 and its code deleted in 0.46.5
// (Rosson 2026-10-23: "we're sticking with threads"). Saved state on this
// client can still name it: a remembered view, a Split side, or the
// "New agents open in" choice. Since 0.46.5 that saved `chat` opens the
// agent's Thread (Rosson 2026-10-27: "Anyone using it as a default point them
// at threads"), the fixed value is written back, and nothing stores `chat`.
// See wiki `Plan - Retirement ledger`.
const LEGACY_CHAT = 'chat'

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
  if (raw === 'terminal' || raw === 'thread' || raw === 'chatter') return raw
  return fallback
}

/** A remembered view, as this client shows it. A saved `chat` (the retired
 *  Chat view) opens the agent's Thread. */
export function parseSessionViewTab(raw: string | null | undefined): SessionViewTab {
  if (raw === 'thread' || raw === 'chatter' || raw === 'split') return raw
  if (raw === LEGACY_CHAT) return 'thread'
  return 'terminal'
}

/** A saved Chat side becomes the Thread; when the other side is already the
 *  Thread it becomes the terminal instead, so the Thread sits beside the
 *  terminal. Both sides `chat` → Terminal | Thread. */
function splitSidesFromRaw(rawLeft: unknown, rawRight: unknown): SessionSplitSides {
  const leftChat = rawLeft === LEGACY_CHAT
  const rightChat = rawRight === LEGACY_CHAT
  if (leftChat && rightChat) return DEFAULT_SPLIT_SIDES
  const left = parseSplitPaneView(rawLeft, DEFAULT_SPLIT_LEFT)
  const right = parseSplitPaneView(rawRight, DEFAULT_SPLIT_RIGHT)
  const besideThread = (other: SplitPaneView): SplitPaneView =>
    other === 'thread' ? 'terminal' : 'thread'
  if (leftChat) return { left: besideThread(right), right }
  if (rightChat) return { left, right: besideThread(left) }
  return { left, right }
}

/** True when a raw saved view (tab string + split JSON) still names the
 *  retired Chat view, so the caller writes the migrated view back. */
export function savedViewNamesLegacyChat(tabRaw: string | null, splitRaw: string | null): boolean {
  if (tabRaw === LEGACY_CHAT) return true
  if (!splitRaw) return false
  try {
    const parsed = JSON.parse(splitRaw) as { left?: unknown; right?: unknown; view?: unknown } | null
    return (
      !!parsed &&
      typeof parsed === 'object' &&
      (parsed.left === LEGACY_CHAT || parsed.right === LEGACY_CHAT || parsed.view === LEGACY_CHAT)
    )
  } catch {
    return false
  }
}

/** Overlay UI is a viewer. Mount Thread/Chatter only while that view is selected.
 *  Split without stored sides is Terminal + Thread. */
export function overlayViewer(
  tab: SessionViewTab,
  sides?: { left?: SplitPaneView | null; right?: SplitPaneView | null } | null,
): {
  thread: boolean
  chatter: boolean
  hidePty: boolean
} {
  const views: SplitPaneView[] =
    tab === 'split'
      ? [
          parseSplitPaneView(sides?.left, DEFAULT_SPLIT_LEFT),
          parseSplitPaneView(sides?.right, DEFAULT_SPLIT_RIGHT),
        ]
      : tab === 'thread' || tab === 'chatter'
        ? [tab]
        : ['terminal']
  return {
    thread: views.includes('thread'),
    chatter: views.includes('chatter'),
    hidePty: !views.includes('terminal'),
  }
}

export function parseSessionSplitSides(raw: string | null | undefined): SessionSplitSides {
  if (!raw) return DEFAULT_SPLIT_SIDES
  try {
    const parsed = JSON.parse(raw) as { left?: unknown; right?: unknown }
    if (!parsed || typeof parsed !== 'object') return DEFAULT_SPLIT_SIDES
    // A saved Chat side (the retired Chat view) opens as the Thread.
    return splitSidesFromRaw(parsed.left, parsed.right)
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
  // Never stores `chat`, even from an untyped caller.
  const safe = splitSidesFromRaw(sides.left, sides.right)
  try {
    localStorage.setItem(
      storageKey,
      JSON.stringify({ left: safe.left, right: safe.right }),
    )
  } catch {
    /* quota / private mode */
  }
}

// ── "New agents open in" (Settings → Workspaces / Agents → Workspace settings)
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

const SPLIT_PANE_ORDER: SplitPaneView[] = ['terminal', 'thread', 'chatter']

/** Coerce anything to a legal pref. Unknown view → Terminal; missing sides →
 *  Terminal | Thread; equal sides → the right side moves to the next view.
 *  A saved Chat view (retired) becomes Thread; a Chat side becomes the Thread
 *  beside the terminal (`splitSidesFromRaw`). */
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
  const migrated = splitSidesFromRaw(sides.left, sides.right)
  const left = migrated.left
  let right = migrated.right
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
    const pref = normalizeNewAgentView(JSON.parse(raw))
    // A saved Chat choice (the retired Chat view) now reads as Thread; write
    // the fixed choice back so `chat` is gone from storage.
    if (savedViewNamesLegacyChat(null, raw)) writeNewAgentView(pref)
    return pref
  } catch {
    return NEW_AGENT_VIEW_DEFAULT
  }
}

export function writeNewAgentView(pref: NewAgentViewPref): void {
  if (typeof localStorage === 'undefined') return
  // Never stores `chat`, even from an untyped caller.
  const safe = normalizeNewAgentView(pref)
  try {
    localStorage.setItem(
      LS_NEW_AGENT_VIEW,
      JSON.stringify({ view: safe.view, left: safe.split.left, right: safe.split.right }),
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
): {
  tab: SessionViewTab
  sides: SessionSplitSides
  fresh: boolean
  /** The saved view named the retired Chat view and was migrated to the
   *  Thread on this read; the caller writes `tab` + `sides` back. */
  migrated: boolean
} {
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
      migrated: savedViewNamesLegacyChat(storedTab, storedSplit),
    }
  }
  const pref = readNewAgentView()
  return {
    tab: pref.view,
    sides: storedSplit !== null ? parseSessionSplitSides(storedSplit) : pref.split,
    fresh: storageKey !== null,
    migrated: savedViewNamesLegacyChat(null, storedSplit),
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
    // Never stores `chat`, even from an untyped caller.
    localStorage.setItem(storageKey, parseSessionViewTab(tab))
  } catch {
    /* quota / private mode */
  }
}
