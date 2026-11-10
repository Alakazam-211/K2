// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { SplitSessionColumns } from './SplitSessionColumns'
import { SplitSearchTargetContext } from './splitChrome'
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

  it("offers the Thread column (else the Chatter's) as the bar's search target", () => {
    const offered: Array<HTMLElement | null> = []
    const offer = (el: HTMLElement | null): void => {
      offered.push(el)
    }
    const { rerender } = render(
      <SplitSearchTargetContext.Provider value={offer}>
        <Frame showSplit left="chatter" right="thread" />
      </SplitSearchTargetContext.Provider>,
    )
    expect(offered.at(-1)).toBe(screen.getByTestId('session-split-right'))
    expect(screen.getByTestId('session-split-right').hasAttribute('data-thread-search-host')).toBe(true)
    expect(screen.getByTestId('session-split-left').hasAttribute('data-thread-search-host')).toBe(false)
    rerender(
      <SplitSearchTargetContext.Provider value={offer}>
        <Frame showSplit left="chatter" right="terminal" />
      </SplitSearchTargetContext.Provider>,
    )
    expect(offered.at(-1)).toBe(screen.getByTestId('session-split-left'))
    rerender(
      <SplitSearchTargetContext.Provider value={offer}>
        <Frame showSplit={false} left="chatter" right="terminal" />
      </SplitSearchTargetContext.Provider>,
    )
    expect(offered.at(-1)).toBeNull()
    expect(document.querySelector('[data-thread-search-host]')).toBeNull()
  })

  it('offers nothing outside a session chrome', () => {
    render(<Frame showSplit left="thread" right="terminal" />)
    expect(document.querySelector('[data-thread-search-host]')).toBeNull()
  })
})
