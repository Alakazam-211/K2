// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { SplitSessionColumns } from './SplitSessionColumns'

// The picture and name read the room; here only where they go matters.
vi.mock('./ThreadViewIdentity', () => ({ ThreadViewIdentity: () => <span data-testid="thread-view-identity" /> }))
import { SplitColumnFloatsContext } from './splitChrome'
import type { SplitPaneView } from './sessionViewTab'

function Frame({
  showSplit,
  left,
  right,
  hidePty = false,
}: {
  showSplit: boolean
  left: SplitPaneView
  right: SplitPaneView
  hidePty?: boolean
}) {
  return (
    <SplitSessionColumns
      showSplit={showSplit}
      hidePty={hidePty}
      left={left}
      right={right}
      terminal={<div data-testid="pty">pty</div>}
      sharedTerminal={<div data-testid="shared">shared</div>}
      renderView={(view) => <div data-testid={`view-${view}`}>{view}</div>}
      single={<div data-testid="single">single</div>}
    />
  )
}

describe('SplitSessionColumns', () => {
  afterEach(() => cleanup())

  it('defaults to Terminal on the left and Thread on the right', () => {
    render(<Frame showSplit left="terminal" right="thread" />)
    expect(screen.getByTestId('session-split-left').contains(screen.getByTestId('pty'))).toBe(true)
    expect(screen.getByTestId('session-split-right').getAttribute('data-split-view')).toBe('thread')
    expect(screen.getByTestId('view-thread')).toBeTruthy()
    expect(screen.queryByTestId('shared')).toBeNull()
    expect(screen.queryByTestId('single')).toBeNull()
  })

  it('allows the same view on both sides and keeps one terminal grid', () => {
    const { rerender } = render(<Frame showSplit left="thread" right="thread" />)
    expect(screen.getAllByTestId('view-thread')).toHaveLength(2)
    expect(screen.getByTestId('pty').parentElement?.style.display).toBe('none')

    rerender(<Frame showSplit left="terminal" right="terminal" />)
    expect(screen.getByTestId('session-split-left').contains(screen.getByTestId('pty'))).toBe(true)
    expect(screen.getByTestId('session-split-right').contains(screen.getByTestId('shared'))).toBe(true)

    rerender(<Frame showSplit left="chatter" right="terminal" />)
    expect(screen.getByTestId('session-split-left').getAttribute('data-split-view')).toBe('chatter')
    expect(screen.getByTestId('session-split-right').contains(screen.getByTestId('pty'))).toBe(true)
  })

  it('hides the side columns when split is not the current view', () => {
    render(<Frame showSplit={false} left="terminal" right="thread" />)
    expect(screen.queryByTestId('session-split-left')).toBeNull()
    expect(screen.queryByTestId('session-split-right')).toBeNull()
    expect(screen.getByTestId('pty').parentElement?.style.display).not.toBe('none')
    expect(screen.getByTestId('single')).toBeTruthy()
  })

  // The terminal's node always comes first in the grid. Placed in column 2
  // with a later column placed in column 1, grid auto-placement wrapped the
  // left column to a second row under the terminal (Thread | Terminal,
  // Chatter | Terminal). Every split column is pinned to the one row.
  for (const [left, right] of [
    ['thread', 'terminal'],
    ['chatter', 'terminal'],
    ['terminal', 'thread'],
    ['thread', 'chatter'],
    ['terminal', 'terminal'],
  ] as const) {
    it(`keeps ${left} | ${right} in one row`, () => {
      render(<Frame showSplit left={left} right={right} />)
      for (const side of ['left', 'right']) {
        const col = screen.getByTestId(`session-split-${side}`)
        expect(col.style.gridRow, side).toBe('1')
        expect(col.style.gridColumn, side).toBe(side === 'left' ? '1' : '2')
      }
    })
  }

  it("draws the Thread's floating objects first in the Thread column, else the Chatter's", () => {
    const source = { name: 'sales', addr: 'sales', bell: <button type="button" data-testid="bell" /> }
    const frame = (left: SplitPaneView, right: SplitPaneView, showSplit = true) => (
      <SplitColumnFloatsContext.Provider value={source}>
        <Frame showSplit={showSplit} left={left} right={right} />
      </SplitColumnFloatsContext.Provider>
    )
    const { rerender } = render(frame('chatter', 'thread'))
    const floatsIn = (side: 'left' | 'right'): Element | null => screen.getByTestId(`session-split-${side}`).querySelector('[data-thread-floats]')
    expect(screen.getByTestId('session-split-right').firstElementChild?.getAttribute('data-split-column-floats')).toBe('thread')
    expect(floatsIn('left')).toBeNull()
    expect(screen.getByTestId('bell')).toBeTruthy()
    rerender(frame('chatter', 'terminal'))
    expect(screen.getByTestId('session-split-left').firstElementChild?.getAttribute('data-split-column-floats')).toBe('chatter')
    // Chatter view has no bell.
    expect(screen.queryByTestId('bell')).toBeNull()
    rerender(frame('terminal', 'terminal'))
    expect(document.querySelector('[data-thread-floats]')).toBeNull()
    rerender(frame('chatter', 'terminal', false))
    expect(document.querySelector('[data-thread-floats]')).toBeNull()
  })

  it('draws none outside a session chrome', () => {
    render(<Frame showSplit left="thread" right="terminal" />)
    expect(document.querySelector('[data-thread-floats]')).toBeNull()
  })
})
