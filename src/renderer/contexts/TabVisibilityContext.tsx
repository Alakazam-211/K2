import { createContext, useContext } from 'react'

/**
 * True when the component's enclosing tab (and, if nested, its enclosing
 * pane item) is currently visible to the user. Retained-view components
 * (CodeMirror, xterm) use this to re-measure on show — they can mount
 * while hidden (parent is display:none), but must measure against real
 * dimensions once visible.
 *
 * Default is `true` for components rendered outside any tab wrapper
 * (e.g., Settings, modals, sidebars) — they're always visible.
 */
export const TabVisibilityContext = createContext<boolean>(true)

/**
 * False when a full-page overlay (Projects / Feedback / Wiki) covers the
 * Agents shell. Default true so dashboard / feedback terminals stay live.
 * Agents TerminalArea + pinned-chat retainer sit under a provider that
 * follows `page === 'agents'` so those sockets drop while Projects is
 * open — leaving Projects then redials one-at-a-time instead of stacking
 * Agents + dashboard grids and ripping the dashboard sockets out at once.
 */
export const PageLiveContext = createContext<boolean>(true)

export function usePageLive(): boolean {
  return useContext(PageLiveContext)
}

export function useIsTabVisible(): boolean {
  const tab = useContext(TabVisibilityContext)
  const page = useContext(PageLiveContext)
  return tab && page
}
