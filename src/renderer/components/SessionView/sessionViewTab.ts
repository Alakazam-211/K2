import { hostScopedKey } from '@/lib/host-scoped-storage'
/** Per-window remembered view (C8). Not daemon-canonical. */

// `chat` (the Chat view: the agent's JSONL transcript drawn over its PTY,
// LIVE 0.41.0) is RETIRED from the UI in 0.46.4 (Rosson 2026-10-23: "we're
// sticking with threads"). Nothing a person can click picks it any more, and
// every read below turns a saved `chat` into the terminal it always was
// (`retireChatTab` / `retireChatSides`). The member stays in these types only
// so the Chat view code (`ChatOverlayColumn`, `ChatOverlayPane`,
// `useChatTranscript`, TerminalPane's chat branches), now reachable only from
// tests, still compiles. 0.46.5: delete `'chat'` from both types along with
// that code. See wiki `Plan - Retirement ledger`.
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

/** A remembered view, as this client shows it. A saved `chat` (Chat view,
 *  retired from the UI in 0.46.4) opens as the terminal. */
export function parseSessionViewTab(raw: string | null | undefined): SessionViewTab {
  if (raw === 'thread' || raw === 'chatter' || raw === 'split') return raw
  return 'terminal'
}

/** 0.46.4 Chat view retirement: the view a saved or requested `chat` becomes.
 *  The Chat view was a face on the agent's own PTY, so it is the terminal.
 *  0.46.5: delete with the `'chat'` type member. */
export function retireChatTab(tab: SessionViewTab): SessionViewTab {
  return tab === 'chat' ? 'terminal' : tab
}

/** 0.46.4 Chat view retirement for the two Split sides: a `chat` side becomes
 *  the terminal, or the Thread when the other side is already the terminal
 *  (`chatFallbackSide`). Both sides `chat` → Terminal | Thread.
 *  0.46.5: delete with the `'chat'` type member. */
export function retireChatSides(sides: SessionSplitSides): SessionSplitSides {
  const { left, right } = sides
  if (left === 'chat' && right === 'chat') return DEFAULT_SPLIT_SIDES
  if (left === 'chat') return { left: chatFallbackSide(right), right }
  if (right === 'chat') return { left, right: chatFallbackSide(left) }
  return sides
}

/** True when a raw saved view (tab string + split JSON) still names the
 *  retired Chat view, so the caller writes the migrated view back.
 *  0.46.5: delete. */
export function savedViewNamesChat(tabRaw: string | null, splitRaw: string | null): boolean {
  if (tabRaw === 'chat') return true
  if (!splitRaw) return false
  try {
    const parsed = JSON.parse(splitRaw) as { left?: unknown; right?: unknown } | null
    return !!parsed && typeof parsed === 'object' && (parsed.left === 'chat' || parsed.right === 'chat')
  } catch {
    return false
  }
}

/** Overlay UI is a viewer. Mount Thread/Chatter/Chat only while that view is selected.
 *  Split without stored sides is Terminal + Thread. The `chat` flag is only
 *  ever true in tests since 0.46.4: no live view state can hold `chat`
 *  (reads and setters go through `retireChatTab` / `retireChatSides`).
 *  0.46.5: drop the `chat` flag. */
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
    // A saved Chat side (retired from the UI in 0.46.4) opens as the terminal
    // or the Thread, never as a blank side.
    return retireChatSides({
      left: parseSplitPaneView(parsed.left, DEFAULT_SPLIT_LEFT),
      right: parseSplitPaneView(parsed.right, DEFAULT_SPLIT_RIGHT),
    })
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

// No `chat`: the Chat view is retired from the UI (0.46.4).
const SPLIT_PANE_ORDER: SplitPaneView[] = ['terminal', 'thread', 'chatter']

/** Coerce anything to a legal pref. Unknown view → Terminal; missing sides →
 *  Terminal | Thread; equal sides → the right side moves to the next view.
 *  A saved Chat view or Chat side (retired 0.46.4) becomes Terminal or the
 *  `retireChatSides` pick. */
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
  const retired = retireChatSides({
    left: parseSplitPaneView(sides.left, DEFAULT_SPLIT_LEFT),
    right: parseSplitPaneView(sides.right, DEFAULT_SPLIT_RIGHT),
  })
  const left = retired.left
  let right = retired.right
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
): {
  tab: SessionViewTab
  sides: SessionSplitSides
  fresh: boolean
  /** The saved view named the retired Chat view (0.46.4) and was migrated
   *  on this read; the caller writes `tab` + `sides` back. 0.46.5: drop. */
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
      migrated: savedViewNamesChat(storedTab, storedSplit),
    }
  }
  const pref = readNewAgentView()
  return {
    tab: pref.view,
    sides: storedSplit !== null ? parseSessionSplitSides(storedSplit) : pref.split,
    fresh: storageKey !== null,
    migrated: savedViewNamesChat(null, storedSplit),
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
