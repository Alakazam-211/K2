// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { SplitSessionColumns } from './SplitSessionColumns'
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

    rerender(<Frame showSplit left="chat" right="terminal" />)
    expect(screen.getByTestId('session-split-left').getAttribute('data-split-view')).toBe('chat')
    expect(screen.getByTestId('session-split-right').contains(screen.getByTestId('pty'))).toBe(true)
  })

  it('hides the side columns when split is not the current view', () => {
    render(<Frame showSplit={false} left="terminal" right="thread" />)
    expect(screen.queryByTestId('session-split-left')).toBeNull()
    expect(screen.queryByTestId('session-split-right')).toBeNull()
    expect(screen.getByTestId('pty').parentElement?.style.display).not.toBe('none')
    expect(screen.getByTestId('single')).toBeTruthy()
  })
})
