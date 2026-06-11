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

export function useIsTabVisible(): boolean {
  return useContext(TabVisibilityContext)
}
