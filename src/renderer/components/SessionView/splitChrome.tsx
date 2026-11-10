import { createContext, useContext, useMemo, useState, type JSX, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'

// Split view has no top bar over the Thread either. The Thread's column gets
// Thread view's floating objects, only those split view keeps
// (SPLIT_VIEW_THREAD_FLOATS in threadFloats.tsx); the view menu, which is how
// a person leaves split view or picks its sides, moves to a short bar over
// the other column. The session's chrome draws both: the columns
// (SplitSessionColumns) offer the two places through this context, and until
// both are there (a terminal that failed to open draws no columns) the
// chrome keeps its usual bar, so the view menu is never lost.

export interface SplitChromeSlots {
  /** The Thread's column offers its floats slot (null when it goes). */
  setThread: (el: HTMLElement | null) => void
  /** The other column offers the view menu's place. */
  setBar: (el: HTMLElement | null) => void
}

export const SplitChromeSlotsContext = createContext<SplitChromeSlots | null>(null)

export function useSplitChromeSlots(): SplitChromeSlots | null {
  return useContext(SplitChromeSlotsContext)
}

/** Split view with a Thread side: the floats go on the first Thread column. */
export function splitThreadSide(
  viewTab: SessionViewTab,
  left: SplitPaneView,
  right: SplitPaneView,
): 'left' | 'right' | null {
  if (viewTab !== 'split') return null
  if (left === 'thread') return 'left'
  if (right === 'thread') return 'right'
  return null
}

/** For the chrome: the context to provide, and the two places once split
 *  view has offered them (else null: draw the header where it is). */
export function useSplitChrome(
  viewTab: SessionViewTab,
  left: SplitPaneView,
  right: SplitPaneView,
): { slots: SplitChromeSlots; placed: { thread: HTMLElement; bar: HTMLElement } | null } {
  const [thread, setThread] = useState<HTMLElement | null>(null)
  const [bar, setBar] = useState<HTMLElement | null>(null)
  const slots = useMemo<SplitChromeSlots>(() => ({ setThread, setBar }), [])
  const placed =
    splitThreadSide(viewTab, left, right) && thread?.isConnected && bar?.isConnected ? { thread, bar } : null
  return { slots, placed }
}

/** The session's header where it belongs: in place (`header(false)`), or in
 *  split view the Thread column's floats (`header(true)`) and the view menu's
 *  bar over the other column. */
export function PlacedSessionHeader({
  placed,
  header,
  viewMenu,
}: {
  placed: { thread: HTMLElement; bar: HTMLElement } | null
  header: (splitColumn: boolean) => ReactNode
  viewMenu: ReactNode
}): JSX.Element {
  if (!placed) return <>{header(false)}</>
  return (
    <>
      {createPortal(header(true), placed.thread)}
      {createPortal(<SplitViewBar>{viewMenu}</SplitViewBar>, placed.bar)}
    </>
  )
}

/** Split view's view menu, over the column beside the Thread's: as tall as
 *  Terminal view's bar, with nothing else on it. */
export function SplitViewBar({ children }: { children: ReactNode }): JSX.Element {
  return (
    <div
      className="h-8 flex-shrink-0 px-3 flex items-stretch min-w-0 border-b border-[var(--color-border)]"
      data-testid="split-view-bar"
    >
      {children}
    </div>
  )
}
