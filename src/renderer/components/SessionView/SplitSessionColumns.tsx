import { useMemo, useRef, type CSSProperties, type ReactNode, type RefObject } from 'react'
import type { SplitPaneView } from './sessionViewTab'
import { useTerminalRevealLayout, type TerminalReveal } from './threadReveal'
import { THREAD_FLOATS_SLOT_ATTR, useThreadFloatsClearOfTerminal } from './threadFloats'
import { splitThreadSide, useSplitChromeSlots } from './splitChrome'

/**
 * Split columns around one terminal node. That node stays the first child
 * so the PTY grid does not remount when the sides change. A second Terminal
 * side cannot host another canvas, so it renders `sharedTerminal`.
 *
 * `reveal` is Thread view's terminal reveal (`threadReveal.ts`): the same
 * node, hidden until asked for, then beside or above the Thread. The Thread
 * stays in `single`, so neither side remounts on a toggle.
 *
 * Thread view's panels (Chatter, Heartbeats: `threadPanels.ts`) slide out of
 * the same place at the same size, in a column of their own after the
 * terminal's (`panel`). One drawer at a time; a swap between two open ones
 * doesn't slide.
 *
 * In split view, under a session chrome (`splitChrome.tsx`), the first Thread
 * column offers a slot for the chrome's floating objects and the other column
 * a place for its view menu, so neither column has a bar over the Thread.
 */
export function SplitSessionColumns({
  showSplit,
  hidePty,
  left,
  right,
  terminal,
  sharedTerminal,
  renderView,
  single,
  reveal,
  panel,
}: {
  showSplit: boolean
  hidePty: boolean
  left: SplitPaneView
  right: SplitPaneView
  terminal: ReactNode
  sharedTerminal: ReactNode
  renderView: (view: Exclude<SplitPaneView, 'terminal'>) => ReactNode
  single: ReactNode
  /** Thread view only. Everywhere else the columns stay as they were. */
  reveal?: TerminalReveal | null
  /** Thread view's open panel, shown when `reveal.kind` is one. */
  panel?: ReactNode
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const singleRef = useRef<HTMLDivElement>(null)
  const drawerKind = reveal?.kind ?? 'terminal'
  const drawerKindRef = useRef(drawerKind)
  drawerKindRef.current = drawerKind
  // The column that slides: the terminal's, or the panel's.
  const drawerRef = useMemo<RefObject<HTMLElement | null>>(
    () => ({
      get current() {
        return drawerKindRef.current === 'terminal' ? terminalRef.current : panelRef.current
      },
    }),
    [],
  )
  const revealLayout = useTerminalRevealLayout(reveal, {
    container: containerRef,
    terminal: drawerRef,
    thread: singleRef,
  })
  const revealShown = !!reveal?.shown
  const terminalRevealShown = revealShown && drawerKind === 'terminal'
  const panelShown = !showSplit && revealShown && drawerKind !== 'terminal'
  // Solid in every theme; never frosted.
  const drawerStyle: CSSProperties = {
    position: 'relative',
    zIndex: 1,
    flex: '0 0 auto',
    background: 'var(--color-bg)',
    ...(revealLayout.placement === 'left'
      ? { width: revealLayout.sizePx, borderRight: '1px solid var(--color-border)' }
      : { height: revealLayout.sizePx, borderBottom: '1px solid var(--color-border)' }),
  }
  useThreadFloatsClearOfTerminal(containerRef, !!reveal, {
    left: reveal?.open && revealLayout.placement === 'left' ? revealLayout.sizePx : 0,
    top: reveal?.open && revealLayout.placement === 'top' ? revealLayout.sizePx : 0,
  })
  const terminalSide: 'left' | 'right' | null = !showSplit
    ? null
    : left === 'terminal'
      ? 'left'
      : right === 'terminal'
        ? 'right'
        : null
  const terminalShown = showSplit ? terminalSide !== null : !hidePty
  const chromeSlots = useSplitChromeSlots()
  const threadSide = chromeSlots ? splitThreadSide(showSplit ? 'split' : 'terminal', left, right) : null
  const barSide = threadSide === null ? null : threadSide === 'left' ? 'right' : 'left'
  // Always the first child of its column, so the column's view never moves.
  const barSlot = (side: 'left' | 'right'): ReactNode =>
    barSide === side ? <div ref={chromeSlots?.setBar} className="flex-shrink-0" data-testid="split-view-bar-slot" /> : null
  const threadSlot = (side: 'left' | 'right'): ReactNode =>
    threadSide === side ? (
      <div ref={chromeSlots?.setThread} style={{ display: 'contents' }} {...{ [THREAD_FLOATS_SLOT_ATTR]: '' }} data-testid="split-thread-floats-slot" />
    ) : null
  const terminalStyle: CSSProperties = {
    display: terminalShown || terminalRevealShown ? 'flex' : 'none',
    flexDirection: 'column',
    minWidth: 0,
    minHeight: 0,
    overflow: 'hidden',
    ...(showSplit
      ? {
          gridColumn: terminalSide === 'right' ? 2 : 1,
          ...(terminalSide === 'right'
            ? { borderLeft: '1px solid var(--color-border)' }
            : {}),
        }
      : terminalRevealShown
        ? drawerStyle
        : { flex: hidePty ? undefined : 1 }),
  }

  return (
    <div
      ref={containerRef}
      data-testid="session-split-columns"
      data-reveal-placement={revealShown ? revealLayout.placement : undefined}
      data-reveal-drawer={revealShown ? drawerKind : undefined}
      style={{
        flex: 1,
        minHeight: 0,
        minWidth: 0,
        position: 'relative',
        display: showSplit ? 'grid' : 'flex',
        gridTemplateColumns: showSplit ? '1fr 1fr' : undefined,
        gridTemplateRows: showSplit ? 'minmax(0, 1fr)' : undefined,
        // The terminal stays the first child, which puts it on top or at the left.
        flexDirection: showSplit
          ? undefined
          : revealShown && revealLayout.placement === 'left'
            ? 'row'
            : 'column',
        overflow: reveal ? 'hidden' : undefined,
      }}
    >
      <div
        ref={terminalRef}
        style={terminalStyle}
        data-testid={terminalSide ? `session-split-${terminalSide}` : reveal ? 'session-reveal-terminal' : undefined}
        data-split-view={terminalSide ? 'terminal' : undefined}
      >
        {terminalSide ? barSlot(terminalSide) : null}
        {terminal}
      </div>
      {panelShown ? (
        <div
          ref={panelRef}
          data-testid="session-reveal-panel"
          data-thread-drawer={drawerKind}
          style={{ ...drawerStyle, display: 'flex', flexDirection: 'column', minWidth: 0, minHeight: 0, overflow: 'hidden' }}
        >
          {panel}
        </div>
      ) : null}
      {showSplit && left !== 'terminal' ? (
        <div
          className="relative min-w-0 min-h-0 overflow-hidden flex flex-col"
          style={{ gridColumn: 1 }}
          data-testid="session-split-left"
          data-split-view={left}
        >
          {threadSlot('left')}
          {barSlot('left')}
          {renderView(left)}
        </div>
      ) : null}
      {showSplit && right !== 'terminal' ? (
        <div
          className="relative min-w-0 min-h-0 overflow-hidden flex flex-col border-l border-[var(--color-border)]"
          style={{ gridColumn: 2 }}
          data-testid="session-split-right"
          data-split-view={right}
        >
          {threadSlot('right')}
          {barSlot('right')}
          {renderView(right)}
        </div>
      ) : null}
      {showSplit && left === 'terminal' && right === 'terminal' ? (
        <div
          className="min-w-0 min-h-0 overflow-hidden flex flex-col border-l border-[var(--color-border)]"
          style={{ gridColumn: 2 }}
          data-testid="session-split-right"
          data-split-view="terminal"
        >
          {sharedTerminal}
        </div>
      ) : null}
      {showSplit ? null : (
        // Always this wrapper, so a toggle never remounts the Thread. No box
        // of its own unless Thread view's reveal needs one to move.
        <div
          ref={singleRef}
          data-testid="session-single-slot"
          style={
            reveal
              ? { position: 'relative', display: 'flex', flexDirection: 'column', flex: 1, minWidth: 0, minHeight: 0 }
              : { display: 'contents' }
          }
        >
          {single}
        </div>
      )}
    </div>
  )
}
