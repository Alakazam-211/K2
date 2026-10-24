// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { useContextMenuStore } from '@/stores/context-menu'
import { SessionViewMenu } from './SessionViewMenu'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'

function viewButtonLabel(): string {
  return (screen.getByTestId('session-view-button').textContent ?? '').replace(/\s+/g, ' ').trim()
}

function menuRow(label: string): HTMLButtonElement {
  const menu = document.querySelector('[data-context-menu]')
  if (!menu) throw new Error(`view menu is closed, wanted ${label}`)
  const row = Array.from(menu.querySelectorAll('button')).find((button) =>
    Array.from(button.querySelectorAll('span')).some((span) => span.textContent === label),
  )
  if (!row) throw new Error(`no menu row ${label}`)
  return row as HTMLButtonElement
}

/** Resolve the open menu with `id`, as if a row with that id were picked,
 *  even when no row has it (the menu store would refuse an unknown id). */
function answerMenu(id: string): void {
  const { onSelect } = useContextMenuStore.getState()
  if (!onSelect) throw new Error('no open menu to answer')
  onSelect(id)
  useContextMenuStore.setState({ isOpen: false, items: [], onSelect: null })
}

function Harness({
  initial = 'terminal' as SessionViewTab,
  initialLeft = 'terminal' as SplitPaneView,
  initialRight = 'thread' as SplitPaneView,
}: {
  initial?: SessionViewTab
  initialLeft?: SplitPaneView
  initialRight?: SplitPaneView
}) {
  const [tab, setTab] = useState<SessionViewTab>(initial)
  const [left, setLeft] = useState<SplitPaneView>(initialLeft)
  const [right, setRight] = useState<SplitPaneView>(initialRight)
  return (
    <>
      <SessionViewMenu
        value={tab}
        splitLeft={left}
        splitRight={right}
        onChange={setTab}
        onSplitLeft={setLeft}
        onSplitRight={setRight}
      />
      <div data-testid="current">{tab}</div>
      <ContextMenu />
    </>
  )
}

describe('View menu', () => {
  afterEach(() => {
    cleanup()
    useContextMenuStore.getState().close()
  })

  it('opens under the View button and does not change the view until a row is chosen', async () => {
    render(<Harness />)
    const button = screen.getByTestId('session-view-button')
    button.getBoundingClientRect = () => ({
      x: 40,
      y: 12,
      left: 40,
      top: 12,
      right: 90,
      bottom: 36,
      width: 50,
      height: 24,
      toJSON() { return {} },
    })
    expect(screen.queryByTestId('session-view-tabs')).toBeNull()
    expect(screen.queryByTestId('session-view-chat')).toBeNull()
    expect(screen.queryByTestId('session-view-split-left')).toBeNull()

    await act(async () => {
      fireEvent.click(button, { clientX: 8, clientY: 9 })
    })
    expect(screen.getByTestId('current').textContent).toBe('terminal')
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(useContextMenuStore.getState().x).toBe(40)
    expect(useContextMenuStore.getState().y).toBe(36)
    // No Chat row: the Chat view is retired from the UI (0.46.4).
    expect(useContextMenuStore.getState().items.map((item) => item.label)).toEqual([
      'Terminal',
      'Thread',
      'Chatter',
      'Split view',
    ])
    expect(useContextMenuStore.getState().items.some((item) => item.badge === 'Beta')).toBe(false)

    await act(async () => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(screen.getByTestId('current').textContent).toBe('terminal')
    expect(useContextMenuStore.getState().isOpen).toBe(false)

    await act(async () => {
      fireEvent.click(button, { clientX: 8, clientY: 9 })
    })
    const backdrop = document.querySelector('[data-context-backdrop]')
    expect(backdrop).toBeTruthy()
    await act(async () => {
      fireEvent.mouseDown(backdrop!)
    })
    expect(screen.getByTestId('current').textContent).toBe('terminal')

    await act(async () => {
      fireEvent.click(button)
    })
    await act(async () => {
      fireEvent.click(menuRow('Thread'))
    })
    expect(screen.getByTestId('current').textContent).toBe('thread')
    expect(screen.queryByTestId('session-view-split-left')).toBeNull()
    expect(viewButtonLabel()).toBe('Thread')
    expect(viewButtonLabel()).not.toBe('View')
  })

  it('names the button after the active mode instead of View', async () => {
    render(<Harness />)
    expect(viewButtonLabel()).toBe('Terminal')
    expect(viewButtonLabel()).not.toBe('View')

    const picks: Array<[string, string]> = [
      ['Thread', 'Thread'],
      ['Chatter', 'Chatter'],
      ['Split view', 'Split view'],
      ['Terminal', 'Terminal'],
    ]
    for (const [row, expected] of picks) {
      await act(async () => {
        fireEvent.click(screen.getByTestId('session-view-button'))
      })
      await act(async () => {
        fireEvent.click(menuRow(row))
      })
      expect(viewButtonLabel()).toBe(expected)
      expect(viewButtonLabel()).not.toBe('View')
    }
    expect(screen.getByTestId('current').textContent).toBe('terminal')
  })

  it('follows value when the mode changes without opening the menu', () => {
    const modes: Array<[SessionViewTab, string]> = [
      ['terminal', 'Terminal'],
      ['thread', 'Thread'],
      ['chatter', 'Chatter'],
      ['split', 'Split view'],
    ]
    const { rerender } = render(
      <SessionViewMenu
        value="terminal"
        splitLeft="terminal"
        splitRight="thread"
        onChange={() => {}}
        onSplitLeft={() => {}}
        onSplitRight={() => {}}
      />,
    )
    for (const [value, expected] of modes) {
      rerender(
        <SessionViewMenu
          value={value}
          splitLeft="terminal"
          splitRight="thread"
            onChange={() => {}}
          onSplitLeft={() => {}}
          onSplitRight={() => {}}
        />,
      )
      expect(viewButtonLabel()).toBe(expected)
      expect(viewButtonLabel()).not.toBe('View')
    }
  })

  it('has no Chat row and ignores a picked chat id (Chat view retired 0.46.4)', async () => {
    render(<Harness />)
    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    const ids = useContextMenuStore.getState().items.map((item) => item.id)
    expect(ids).not.toContain('chat')
    expect(() => menuRow('Chat')).toThrow('no menu row Chat')
    // A stray `chat` answer from the menu store (no row offers it) is ignored.
    await act(async () => {
      answerMenu('chat')
    })
    expect(screen.getByTestId('current').textContent).toBe('terminal')
  })

  it('shows Terminal and Thread choosers only while Split view is selected', async () => {
    render(<Harness />)
    expect(screen.queryByTestId('session-view-split-left')).toBeNull()
    expect(screen.queryByTestId('session-view-split-right')).toBeNull()

    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-button'))
    })
    await act(async () => {
      fireEvent.click(menuRow('Split view'))
    })
    expect(screen.getByTestId('current').textContent).toBe('split')
    const left = screen.getByTestId('session-view-split-left')
    const right = screen.getByTestId('session-view-split-right')
    expect(left.textContent).toContain('Terminal')
    expect(right.textContent).toContain('Thread')
    expect(screen.getByTestId('session-view-menu').getAttribute('data-split-left')).toBe('terminal')
    expect(screen.getByTestId('session-view-menu').getAttribute('data-split-right')).toBe('thread')

    await act(async () => {
      fireEvent.click(left)
    })
    expect(useContextMenuStore.getState().items.map((item) => item.label)).toEqual([
      'Terminal',
      'Thread',
      'Chatter',
    ])
    await act(async () => {
      fireEvent.click(menuRow('Chatter'))
    })
    expect(screen.getByTestId('session-view-split-left').textContent).toContain('Chatter')
    expect(screen.getByTestId('session-view-split-right').textContent).toContain('Thread')

    await act(async () => {
      fireEvent.click(screen.getByTestId('session-view-split-right'))
    })
    await act(async () => {
      fireEvent.click(menuRow('Chatter'))
    })
    expect(screen.getByTestId('session-view-split-left').textContent).toContain('Chatter')
    expect(screen.getByTestId('session-view-split-right').textContent).toContain('Chatter')
  })

  it('a split chooser has no Chat row and ignores a picked chat id', async () => {
    render(<Harness initial="split" />)
    for (const side of ['session-view-split-left', 'session-view-split-right']) {
      await act(async () => {
        fireEvent.click(screen.getByTestId(side))
      })
      expect(useContextMenuStore.getState().items.map((item) => item.id)).toEqual([
        'terminal',
        'thread',
        'chatter',
      ])
      expect(() => menuRow('Chat')).toThrow('no menu row Chat')
      await act(async () => {
        answerMenu('chat')
      })
    }
    expect(screen.getByTestId('session-view-menu').getAttribute('data-split-left')).toBe('terminal')
    expect(screen.getByTestId('session-view-menu').getAttribute('data-split-right')).toBe('thread')
  })
})
