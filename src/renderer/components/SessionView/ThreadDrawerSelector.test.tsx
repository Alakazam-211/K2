// @vitest-environment jsdom
// Thread view's drawer selector: the terminal, Chatter and Heartbeats in one
// capsule, and the terminal's chord (prd-thread-view-focus-v1 §6).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useState } from 'react'
import { act, cleanup, fireEvent, renderHook, screen } from '@testing-library/react'

const windowRoom = vi.hoisted(() => ({ focused: true }))
vi.mock('@/stores/window-room', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/stores/window-room')>()),
  isFocusedRoom: () => windowRoom.focused,
}))
vi.mock('@/lib/zen/zen-view', () => ({ zenShownNow: () => false }))

import { HOTKEYS, RESERVED_KEYS, getDefaultKeybindings, keyEventMatchesCombo } from '@shared/hotkeys'
import { useSettingsStore } from '@/stores/settings'
import { useTerminalShortcuts } from '@/hooks/useTerminalShortcuts'
import type { Room } from '@/stores/room'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { ThreadDrawerSelector } from './ThreadDrawerSelector'
import type { ThreadPanelKind } from './threadPanels'
import { registerRevealTarget, revealTargetsForTests } from './threadReveal'

const room = testRoom({ tabs: {}, scope: primaryScope() })

beforeEach(() => {
  windowRoom.focused = true
  revealTargetsForTests().clear()
  useSettingsStore.setState({ keybindings: {} })
})

afterEach(() => {
  cleanup()
  revealTargetsForTests().clear()
})

describe('the Ctrl+` binding', () => {
  it('is on every platform and clashes with no other hotkey or reserved key', () => {
    expect(getDefaultKeybindings(true).toggleThreadTerminal).toBe('Ctrl+`')
    expect(getDefaultKeybindings(false).toggleThreadTerminal).toBe('Ctrl+`')
    const others = HOTKEYS.filter((h) => h.id !== 'toggleThreadTerminal')
    expect(others.some((h) => h.defaultKey === 'Ctrl+`' || h.defaultKeyOther === 'Ctrl+`')).toBe(false)
    expect((RESERVED_KEYS as readonly string[]).includes('Ctrl+`')).toBe(false)
  })

  it('matches only the chord itself', () => {
    const ev = (over: Partial<KeyboardEventInit>) =>
      new KeyboardEvent('keydown', { key: '`', code: 'Backquote', ...over })
    expect(keyEventMatchesCombo(ev({ ctrlKey: true }), 'Ctrl+`')).toBe(true)
    expect(keyEventMatchesCombo(ev({}), 'Ctrl+`')).toBe(false)
    expect(keyEventMatchesCombo(ev({ ctrlKey: true, shiftKey: true }), 'Ctrl+`')).toBe(false)
    expect(keyEventMatchesCombo(ev({ metaKey: true }), 'Ctrl+`')).toBe(false)
  })
})

describe('useTerminalShortcuts: the chord', () => {
  const tabsRoom = { scope: primaryScope(), tabs: { getState: () => ({ activeGroupIndex: 0 }) } } as unknown as Room

  function press(over: Partial<KeyboardEventInit> = {}): KeyboardEvent {
    const ev = new KeyboardEvent('keydown', { key: '`', code: 'Backquote', ctrlKey: true, bubbles: true, cancelable: true, ...over })
    window.dispatchEvent(ev)
    return ev
  }

  function target(toggled: string[]): void {
    const element = document.createElement('button')
    element.getClientRects = (() => [{}]) as unknown as typeof element.getClientRects
    document.body.appendChild(element)
    registerRevealTarget({ room: tabsRoom, element, enabled: () => true, toggle: () => toggled.push('t') })
  }

  it('toggles the Thread view pane and takes the key', () => {
    const toggled: string[] = []
    target(toggled)
    renderHook(() => useTerminalShortcuts(tabsRoom, '/w'))
    const ev = press()
    expect(toggled).toEqual(['t'])
    expect(ev.defaultPrevented).toBe(true)
  })

  it('leaves the key alone with no Thread view (Terminal view keeps it)', () => {
    renderHook(() => useTerminalShortcuts(tabsRoom, '/w'))
    expect(press().defaultPrevented).toBe(false)
  })

  it('does nothing in a room that is not focused', () => {
    windowRoom.focused = false
    const toggled: string[] = []
    target(toggled)
    renderHook(() => useTerminalShortcuts(tabsRoom, '/w'))
    press()
    expect(toggled).toEqual([])
  })

  it('a held key does not flap', () => {
    const toggled: string[] = []
    target(toggled)
    renderHook(() => useTerminalShortcuts(tabsRoom, '/w'))
    press({ repeat: true })
    expect(toggled).toEqual([])
  })

  it('follows a rebind', () => {
    useSettingsStore.setState({ keybindings: { toggleThreadTerminal: 'Ctrl+J' } })
    const toggled: string[] = []
    target(toggled)
    renderHook(() => useTerminalShortcuts(tabsRoom, '/w'))
    press()
    expect(toggled).toEqual([])
    press({ key: 'J', code: 'KeyJ' })
    expect(toggled).toEqual(['t'])
  })
})

/** Wired as the headers wire it (`useSessionViewTab`): revealing the
 *  terminal closes a panel, opening a panel hides the terminal. */
function Selector({
  available = true,
  revealed: startRevealed = false,
  panels = true,
  onToggle,
}: {
  available?: boolean
  revealed?: boolean
  panels?: boolean
  onToggle?: () => void
}): React.JSX.Element {
  const [revealed, setRevealed] = useState(startRevealed)
  const [panel, setPanel] = useState<ThreadPanelKind | null>(null)
  return (
    <ThreadDrawerSelector
      terminalRevealed={revealed}
      terminalAvailable={available}
      onToggleTerminal={() => {
        onToggle?.()
        if (!revealed) setPanel(null)
        setRevealed(!revealed)
      }}
      panel={panel}
      onPanel={
        panels
          ? (next) => {
              setPanel(next)
              if (next) setRevealed(false)
            }
          : undefined
      }
    />
  )
}

const seg = (id: string): HTMLButtonElement => screen.getByTestId(id) as HTMLButtonElement
const pressed = (): string[] =>
  Array.from(document.querySelectorAll('[data-thread-drawer-segment]'))
    .filter((el) => el.getAttribute('aria-pressed') === 'true')
    .map((el) => el.getAttribute('data-thread-drawer-segment') ?? '')

describe('ThreadDrawerSelector', () => {
  it('is one capsule of three segments: Terminal, Chatter, Heartbeats', () => {
    renderInRoom(room, <Selector />)
    const group = screen.getByRole('toolbar', { name: 'Side panels' })
    expect(group.getAttribute('data-k2-float')).toBe('capsule')
    expect(group.getAttribute('data-k2-chamfer')).toBe('control')
    expect(group.getAttribute('data-material')).toBe('frost')
    expect(group.getAttribute('data-frost-shape')).toBe('pill')
    expect(group.hasAttribute('data-page-tabs')).toBe(true)
    const segments = Array.from(group.querySelectorAll('button'))
    expect(segments.map((b) => b.getAttribute('data-thread-drawer-segment'))).toEqual(['terminal', 'chatter', 'heartbeats'])
    for (const b of segments) {
      expect(b.hasAttribute('data-page-tab')).toBe(true)
      expect(b.hasAttribute('data-k2-float')).toBe(false)
      expect(b.getAttribute('type')).toBe('button')
      expect(b.getAttribute('aria-pressed')).toBe('false')
    }
  })

  it('labels each segment, with a tooltip', () => {
    renderInRoom(room, <Selector />)
    expect(seg('thread-terminal-toggle').getAttribute('aria-label')).toBe('Reveal terminal')
    expect(seg('thread-terminal-toggle').title).toContain('Reveal terminal')
    expect(seg('thread-panel-toggle-chatter').getAttribute('aria-label')).toBe('Show Chatter')
    expect(seg('thread-panel-toggle-chatter').title).toBe('Chatter: messages between agents')
    expect(seg('thread-panel-toggle-heartbeats').getAttribute('aria-label')).toBe('Show Heartbeats')
    expect(seg('thread-panel-toggle-heartbeats').title).toBe('Heartbeats: recent heartbeats and other outside input')
  })

  it('selects the open drawer; the open one closes it; another switches', () => {
    renderInRoom(room, <Selector />)
    fireEvent.click(seg('thread-panel-toggle-chatter'))
    expect(pressed()).toEqual(['chatter'])
    expect(seg('thread-panel-toggle-chatter').hasAttribute('data-selected')).toBe(true)
    expect(seg('thread-panel-toggle-chatter').getAttribute('aria-label')).toBe('Hide Chatter')
    fireEvent.click(seg('thread-panel-toggle-heartbeats'))
    expect(pressed()).toEqual(['heartbeats'])
    fireEvent.click(seg('thread-terminal-toggle'))
    expect(pressed()).toEqual(['terminal'])
    expect(seg('thread-terminal-toggle').getAttribute('aria-label')).toBe('Hide terminal')
    fireEvent.click(seg('thread-panel-toggle-chatter'))
    expect(pressed()).toEqual(['chatter'])
    fireEvent.click(seg('thread-panel-toggle-chatter'))
    expect(pressed()).toEqual([])
    fireEvent.click(seg('thread-terminal-toggle'))
    fireEvent.click(seg('thread-terminal-toggle'))
    expect(pressed()).toEqual([])
  })

  it('takes one Tab stop: the open drawer, else the first segment that works', () => {
    renderInRoom(room, <Selector />)
    const stops = (): string[] =>
      Array.from(document.querySelectorAll<HTMLElement>('[data-thread-drawer-segment]'))
        .filter((el) => el.tabIndex === 0)
        .map((el) => el.dataset.threadDrawerSegment ?? '')
    expect(stops()).toEqual(['terminal'])
    fireEvent.click(seg('thread-panel-toggle-heartbeats'))
    expect(stops()).toEqual(['heartbeats'])
    cleanup()
    renderInRoom(room, <Selector available={false} />)
    expect(stops()).toEqual(['chatter'])
  })

  it('arrows move focus round the segments without opening anything', () => {
    renderInRoom(room, <Selector />)
    seg('thread-terminal-toggle').focus()
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(seg('thread-panel-toggle-chatter'))
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(seg('thread-panel-toggle-heartbeats'))
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(seg('thread-terminal-toggle'))
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowLeft' })
    expect(document.activeElement).toBe(seg('thread-panel-toggle-heartbeats'))
    expect(pressed()).toEqual([])
  })

  it('arrows skip the terminal while it is not running', () => {
    renderInRoom(room, <Selector available={false} />)
    seg('thread-panel-toggle-heartbeats').focus()
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(seg('thread-panel-toggle-chatter'))
  })

  it('with no panels, holds the terminal only', () => {
    renderInRoom(room, <Selector panels={false} />)
    expect(Array.from(document.querySelectorAll('[data-thread-drawer-segment]')).map((el) => el.getAttribute('data-thread-drawer-segment'))).toEqual(['terminal'])
  })
})

describe('the terminal segment', () => {
  it('toggles on click and says its state', () => {
    const onToggle = vi.fn()
    renderInRoom(room, <Selector onToggle={onToggle} />)
    const button = seg('thread-terminal-toggle')
    expect(button.getAttribute('aria-pressed')).toBe('false')
    expect(button.getAttribute('aria-label')).toBe('Reveal terminal')
    fireEvent.click(button)
    expect(onToggle).toHaveBeenCalledTimes(1)
  })

  it('names the shortcut on hover', () => {
    renderInRoom(room, <Selector />)
    expect(seg('thread-terminal-toggle').title).toContain('`')
  })

  it('is disabled with no terminal, and the chord then does nothing', () => {
    renderInRoom(room, <Selector available={false} />)
    expect(seg('thread-terminal-toggle').disabled).toBe(true)
    expect(seg('thread-terminal-toggle').title).toBe('No terminal is running yet')
    const entry = Array.from(revealTargetsForTests())[0]
    expect(entry.enabled()).toBe(false)
  })

  it('the chord finds it and toggles the terminal', () => {
    const onToggle = vi.fn()
    renderInRoom(room, <Selector onToggle={onToggle} />)
    const entry = Array.from(revealTargetsForTests())[0]
    expect(entry.element).toBe(seg('thread-terminal-toggle'))
    act(() => entry.toggle())
    expect(onToggle).toHaveBeenCalledTimes(1)
    expect(pressed()).toEqual(['terminal'])
  })

  it('a click keeps focus where it is', () => {
    renderInRoom(room, <Selector revealed />)
    const ev = new MouseEvent('mousedown', { bubbles: true, cancelable: true })
    act(() => {
      seg('thread-terminal-toggle').dispatchEvent(ev)
    })
    expect(ev.defaultPrevented).toBe(true)
  })

  it('registers while mounted and goes away after', () => {
    const { unmount } = renderInRoom(room, <Selector />)
    expect(revealTargetsForTests().size).toBe(1)
    unmount()
    expect(revealTargetsForTests().size).toBe(0)
  })
})
