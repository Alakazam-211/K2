import { createContext, useContext, useState } from 'react'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'

// Split view has one solid bar across both columns, at the top of the pane,
// and nothing floats over either column (Rosson, 2026-11-10). The bar is the
// session's own header, drawn in place (AgentSessionChrome, ChatHeader): the
// view menu and the side choosers on the left, the agent's picture and name
// in the middle, and search, the bell, Token, the session and restart on the
// right. Both columns start below it, in either order.
//
// Search on the bar searches the Thread side, else the Chatter side. That
// column (SplitSessionColumns) offers itself through this context and is
// marked as the search's host (SPLIT_SEARCH_HOST_ATTR), so the pane inside
// it and the bar's field share one search.

/** On the split bar (the header in split view). */
export const SPLIT_BAR_ATTR = 'data-split-bar'
/** On the column the split bar's search searches. */
export const SPLIT_SEARCH_HOST_ATTR = 'data-thread-search-host'

/** What the column offers: itself, or null when it goes. */
export type SplitSearchOffer = (el: HTMLElement | null) => void

export const SplitSearchTargetContext = createContext<SplitSearchOffer | null>(null)

export function useSplitSearchOffer(): SplitSearchOffer | null {
  return useContext(SplitSearchTargetContext)
}

/** The side the split bar's search searches: the first Thread side, else the
 *  first Chatter side. Terminals only: none. */
export function splitSearchSide(
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

/** What that side shows. */
export function splitSearchKind(left: SplitPaneView, right: SplitPaneView): 'thread' | 'chatter' | null {
  const side = splitSearchSide('split', left, right)
  if (!side) return null
  return (side === 'left' ? left : right) as 'thread' | 'chatter'
}

/** For the chrome: the offer to provide to the columns, and the column the
 *  bar's search searches once it is there (else null: no search). */
export function useSplitSearchTarget(): { offer: SplitSearchOffer; host: HTMLElement | null } {
  const [host, offer] = useState<HTMLElement | null>(null)
  return { offer, host: host?.isConnected ? host : null }
}
