// The desktop Thread's activity drawer sits on its message box's top edge,
// like the Zen Thread widget's (Garden 1) and K2 Companion's. The Thread
// list knows the live turn; the box (TerminalComposeBar, built by the
// terminal pane) draws the drawer. `ThreadOverlayColumn` holds both and hands
// the turn (and the Stop: Esc to the agent's PTY) down through this context.

import { createContext, useContext } from 'react'
import type { ThreadTurn } from './overlayThread'

export interface ThreadComposeDrawer {
  turn: ThreadTurn | null
  /** Sends Esc to the agent's session. Absent: no Stop (view-only). */
  onStop?: () => void
}

export const ThreadComposeDrawerContext = createContext<ThreadComposeDrawer | null>(null)

/** The drawer for a message box under a desktop Thread, or null (no Thread
 *  column around this box). */
export function useThreadComposeDrawer(): ThreadComposeDrawer | null {
  return useContext(ThreadComposeDrawerContext)
}
