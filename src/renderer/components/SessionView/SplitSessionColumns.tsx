import { useRef, type CSSProperties, type ReactNode } from 'react'
import type { SplitPaneView } from './sessionViewTab'
import { useTerminalRevealLayout, type TerminalReveal } from './threadReveal'
import { useThreadFloatsClearOfTerminal } from './threadFloats'

/**
 * Split columns around one terminal node. That node stays the first child
 * so the PTY grid does not remount when the sides change. A second Terminal
 * side cannot host another canvas, so it renders `sharedTerminal`.
 *
 * `reveal` is Thread view's terminal reveal (`threadReveal.ts`): the same
 * node, hidden until asked for, then beside or above the Thread. The Thread
 * stays in `single`, so neither side remounts on a toggle.
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
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<HTMLDivElement>(null)
  const singleRef = useRef<HTMLDivElement>(null)
  const revealLayout = useTerminalRevealLayout(reveal, {
    container: containerRef,
    terminal: terminalRef,
    thread: singleRef,
  })
  const revealShown = !!reveal?.shown
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
  const terminalStyle: CSSProperties = {
    display: terminalShown || revealShown ? 'flex' : 'none',
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
      : revealShown
        ? {
            position: 'relative',
            zIndex: 1,
            flex: '0 0 auto',
            // Solid in every theme; never frosted.
            background: 'var(--color-bg)',
            ...(revealLayout.placement === 'left'
              ? { width: revealLayout.sizePx, borderRight: '1px solid var(--color-border)' }
              : { height: revealLayout.sizePx, borderBottom: '1px solid var(--color-border)' }),
          }
        : { flex: hidePty ? undefined : 1 }),
  }

  return (
    <div
      ref={containerRef}
      data-testid="session-split-columns"
      data-reveal-placement={revealShown ? revealLayout.placement : undefined}
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
        {terminal}
      </div>
      {showSplit && left !== 'terminal' ? (
        <div
          className="min-w-0 min-h-0 overflow-hidden flex flex-col"
          style={{ gridColumn: 1 }}
          data-testid="session-split-left"
          data-split-view={left}
        >
          {renderView(left)}
        </div>
      ) : null}
      {showSplit && right !== 'terminal' ? (
        <div
          className="min-w-0 min-h-0 overflow-hidden flex flex-col border-l border-[var(--color-border)]"
          style={{ gridColumn: 2 }}
          data-testid="session-split-right"
          data-split-view={right}
        >
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
