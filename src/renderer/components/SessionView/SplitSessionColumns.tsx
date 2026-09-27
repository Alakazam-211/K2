import type { CSSProperties, ReactNode } from 'react'
import type { SplitPaneView } from './sessionViewTab'

/**
 * Split columns around one terminal node. That node stays the first child
 * so the PTY grid does not remount when the sides change. A second Terminal
 * side cannot host another canvas, so it renders `sharedTerminal`.
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
}: {
  showSplit: boolean
  hidePty: boolean
  left: SplitPaneView
  right: SplitPaneView
  terminal: ReactNode
  sharedTerminal: ReactNode
  renderView: (view: Exclude<SplitPaneView, 'terminal'>) => ReactNode
  single: ReactNode
}): React.JSX.Element {
  const terminalSide: 'left' | 'right' | null = !showSplit
    ? null
    : left === 'terminal'
      ? 'left'
      : right === 'terminal'
        ? 'right'
        : null
  const terminalShown = showSplit ? terminalSide !== null : !hidePty
  const terminalStyle: CSSProperties = {
    display: terminalShown ? 'flex' : 'none',
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
      : { flex: hidePty ? undefined : 1 }),
  }

  return (
    <div
      data-testid="session-split-columns"
      style={{
        flex: 1,
        minHeight: 0,
        minWidth: 0,
        position: 'relative',
        display: showSplit ? 'grid' : 'flex',
        gridTemplateColumns: showSplit ? '1fr 1fr' : undefined,
        gridTemplateRows: showSplit ? 'minmax(0, 1fr)' : undefined,
        flexDirection: showSplit ? undefined : 'column',
      }}
    >
      <div
        style={terminalStyle}
        data-testid={terminalSide ? `session-split-${terminalSide}` : undefined}
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
      {showSplit ? null : single}
    </div>
  )
}
