// @vitest-environment jsdom
// The Terminal toggle and its chord (prd-thread-view-focus-v1 §6).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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
import { ThreadTerminalToggle } from './ThreadTerminalToggle'
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

describe('ThreadTerminalToggle', () => {
  it('toggles on click and says its state', () => {
    const onToggle = vi.fn()
    renderInRoom(room, <ThreadTerminalToggle revealed={false} disabled={false} onToggle={onToggle} />)
    const button = screen.getByTestId('thread-terminal-toggle')
    expect(button.getAttribute('aria-pressed')).toBe('false')
    expect(button.getAttribute('aria-label')).toBe('Reveal terminal')
    fireEvent.click(button)
    expect(onToggle).toHaveBeenCalledTimes(1)
  })

  it('names the shortcut on hover', () => {
    renderInRoom(room, <ThreadTerminalToggle revealed={false} disabled={false} onToggle={vi.fn()} />)
    expect(screen.getByTestId('thread-terminal-toggle').title).toContain('`')
  })

  it('is disabled with no terminal, and the chord then does nothing', () => {
    renderInRoom(room, <ThreadTerminalToggle revealed={false} disabled onToggle={vi.fn()} />)
    expect((screen.getByTestId('thread-terminal-toggle') as HTMLButtonElement).disabled).toBe(true)
    const entry = Array.from(revealTargetsForTests())[0]
    expect(entry.enabled()).toBe(false)
  })

  it('a click keeps focus where it is', () => {
    renderInRoom(room, <ThreadTerminalToggle revealed disabled={false} onToggle={vi.fn()} />)
    const ev = new MouseEvent('mousedown', { bubbles: true, cancelable: true })
    act(() => {
      screen.getByTestId('thread-terminal-toggle').dispatchEvent(ev)
    })
    expect(ev.defaultPrevented).toBe(true)
  })

  it('registers while mounted and goes away after', () => {
    const { unmount } = renderInRoom(room, <ThreadTerminalToggle revealed={false} disabled={false} onToggle={vi.fn()} />)
    expect(revealTargetsForTests().size).toBe(1)
    unmount()
    expect(revealTargetsForTests().size).toBe(0)
  })
})
