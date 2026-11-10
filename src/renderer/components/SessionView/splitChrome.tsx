import { createContext, useContext, type ReactNode } from 'react'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'

// Split view: one solid bar across both columns at the top of the pane, and
// the Thread's own objects in the Thread (Rosson, 2026-11-10).
//
// - The bar is the session's own header, drawn in place (AgentSessionChrome,
//   ChatHeader): the view menu and the side choosers on the left; the
//   eye/pencil (with a terminal side), Token, the session and restart on the
//   right. Both columns start below it, in either order.
// - The Thread's column keeps Thread view's floating objects that belong to
//   the Thread: the agent's picture and name, search and the bell
//   (SPLIT_VIEW_THREAD_FLOATS in threadFloats.tsx), drawn by the column
//   itself (SplitColumnFloats). With no Thread side, the first Chatter
//   column takes the picture and name and search (Chatter view has no bell).
//
// The chrome hands the column what it needs through this context: who the
// agent is and the workspace's bell.

export interface SplitColumnFloatsSource {
  /** The agent's display name and its Thread's address (the picture). */
  name: string
  addr: string
  /** The workspace's completion bell, as a floating object (null: none). */
  bell: ReactNode
}

export const SplitColumnFloatsContext = createContext<SplitColumnFloatsSource | null>(null)

export function useSplitColumnFloatsSource(): SplitColumnFloatsSource | null {
  return useContext(SplitColumnFloatsContext)
}

/** On the split bar (the header in split view). */
export const SPLIT_BAR_ATTR = 'data-split-bar'

/** The column that carries the Thread's floating objects: the first Thread
 *  side, else the first Chatter side. Terminals only: none. */
export function splitFloatsSide(
  viewTab: SessionViewTab,
  left: SplitPaneView,
  right: SplitPaneView,
): 'left' | 'right' | null {
  if (viewTab !== 'split') return null
  if (left === 'thread') return 'left'
  if (right === 'thread') return 'right'
  if (left === 'chatter') return 'left'
  if (right === 'chatter') return 'right'
  return null
}

/** What that column shows. */
export function splitFloatsKind(left: SplitPaneView, right: SplitPaneView): 'thread' | 'chatter' | null {
  const side = splitFloatsSide('split', left, right)
  if (!side) return null
  return (side === 'left' ? left : right) as 'thread' | 'chatter'
}
