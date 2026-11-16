// @vitest-environment jsdom
// ⌥⌘↑ / ⌥⌘↓ on Home (Rosson 2026-11-16): the Home nav's row above / below
// the selected one, in the order the nav shows them (pins first, most
// recent first when on); the ends stop; with nothing selected ↓ takes the
// first row and ↑ the last; a code editor or a dialog keeps the keys; off
// macOS the keys are Ctrl+Alt+Up / Down.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

const h = vi.hoisted(() => ({
  mac: true,
  openHomeRow: vi.fn(),
  closed: new Set<string>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/zen/zen-view', async (orig) => ({
  ...(await orig<typeof import('@/lib/zen/zen-view')>()),
  zenShownNow: () => false,
  zenOnNow: () => false,
}))
vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => h.mac,
}))
vi.mock('@/lib/remote-rooms-preview', async (orig) => ({
  ...(await orig<typeof import('@/lib/remote-rooms-preview')>()),
  remoteRoomsPreviewEnabled: () => true,
}))
vi.mock('@/lib/home-open', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-open')>()),
  openHomeRow: h.openHomeRow,
}))
vi.mock('@/components/Home/home-room', async (orig) => ({
  ...(await orig<typeof import('@/components/Home/home-room')>()),
  homeRowOpenableNow: (row: { address: string }) => !h.closed.has(row.address),
}))

import { useHomeRowStepShortcut } from '@/hooks/useHomeRowStepShortcut'
import { HOME_ROW_STEP_IDS, homeRowStepForEvent, nextHomeRowIndex } from './home-row-step'
import { HOTKEYS, getDefaultKeybindings, formatKeyCombo } from '@shared/hotkeys'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useHomesStore } from '@/stores/homes'

const ROWS = ['a::local', 'b::local', 'c::local', 'd::local']

/** Exactly these groups (the store keeps at least one), each with rows. */
function setGroups(groups: Array<{ name: string; rows: string[] }>): string[] {
  const st = useHomesStore.getState()
  const before = st.homes.map((x) => x.id)
  const ids: string[] = []
  for (const g of groups) {
    const id = st.createHome(g.name)
    if (!id) throw new Error(`createHome refused ${g.name}`)
    for (const a of g.rows) if (!useHomesStore.getState().addRow(id, { address: a, workspaceId: null, label: a })) throw new Error(`addRow ${a}`)
    ids.push(id)
  }
  for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error('deleteHome refused')
  return ids
}

type Arrow = 'ArrowUp' | 'ArrowDown'

/** macOS ⌥⌘ (default) or off-macOS Ctrl+Alt, dispatched at `target`. */
function press(key: Arrow, opts: { mods?: Partial<KeyboardEventInit>; target?: EventTarget; repeat?: boolean } = {}): KeyboardEvent {
  const mods = opts.mods ?? (h.mac ? { metaKey: true, altKey: true } : { ctrlKey: true, altKey: true })
  const ev = new KeyboardEvent('keydown', { key: key, code: key, bubbles: true, cancelable: true, repeat: opts.repeat, ...mods })
  act(() => {
    ;(opts.target ?? window).dispatchEvent(ev)
  })
  return ev
}

function opened(): string[] {
  return h.openHomeRow.mock.calls.map((c) => (c[0] as { address: string }).address)
}

function shown(): string | null {
  return useHomeRoomsStore.getState().shown
}

let ids: string[] = []

beforeEach(() => {
  localStorage.clear()
  sessionStorage.clear()
  h.mac = true
  h.closed.clear()
  // Opening a row puts its room on screen, as a click does (remote room).
  h.openHomeRow.mockReset()
  h.openHomeRow.mockImplementation((row: { address: string }) => {
    useHomeRoomsStore.setState({ shown: row.address })
    return 'room'
  })
  useHomeRoomsStore.setState({ shown: null })
  useSettingsStore.setState({ keybindings: {}, settingsOpen: false })
  usePageViewStore.setState({ page: 'home' })
  ids = setGroups([
    { name: 'Work', rows: ROWS },
    { name: 'Play', rows: ['x::local', 'y::local'] },
  ])
  act(() => useHomesStore.getState().selectHome(ids[0]))
  renderHook(() => useHomeRowStepShortcut())
})

afterEach(() => {
  cleanup()
  document.body.innerHTML = ''
})

describe('the bindings', () => {
  it('⌥⌘↑ / ⌥⌘↓ on macOS, Ctrl+Alt+Up / Down elsewhere, under Navigation', () => {
    const mac = getDefaultKeybindings(true)
    const other = getDefaultKeybindings(false)
    expect(mac[HOME_ROW_STEP_IDS.up]).toBe('Meta+Alt+ArrowUp')
    expect(mac[HOME_ROW_STEP_IDS.down]).toBe('Meta+Alt+ArrowDown')
    expect(other[HOME_ROW_STEP_IDS.up]).toBe('Ctrl+Alt+ArrowUp')
    expect(other[HOME_ROW_STEP_IDS.down]).toBe('Ctrl+Alt+ArrowDown')
    expect(formatKeyCombo(mac[HOME_ROW_STEP_IDS.up], true)).toBe('⌘⌥↑')
    expect(formatKeyCombo(other[HOME_ROW_STEP_IDS.down], false)).toBe('Ctrl+Alt+↓')
    for (const id of Object.values(HOME_ROW_STEP_IDS)) {
      expect(HOTKEYS.find((k) => k.id === id)?.category).toBe('Navigation')
    }
  })

  it('no other hotkey has these keys by default', () => {
    for (const isMac of [true, false]) {
      const defaults = getDefaultKeybindings(isMac)
      const ours = new Set(Object.values(HOME_ROW_STEP_IDS).map((id) => defaults[id]))
      const clashes = Object.entries(defaults).filter(([id, combo]) => ours.has(combo) && !Object.values(HOME_ROW_STEP_IDS).includes(id as never))
      expect(clashes).toEqual([])
    }
  })

  it('matches only its own modifiers (not ⌥↑, ⌘↑, ⌥⌘⇧↑ or ⌥⌘←)', () => {
    const ev = (key: string, over: Partial<KeyboardEventInit>) => new KeyboardEvent('keydown', { key, code: key, ...over })
    expect(homeRowStepForEvent(ev('ArrowUp', { metaKey: true, altKey: true }), {}, true)).toBe(-1)
    expect(homeRowStepForEvent(ev('ArrowDown', { metaKey: true, altKey: true }), {}, true)).toBe(1)
    expect(homeRowStepForEvent(ev('ArrowUp', { altKey: true }), {}, true)).toBeNull()
    expect(homeRowStepForEvent(ev('ArrowUp', { metaKey: true }), {}, true)).toBeNull()
    expect(homeRowStepForEvent(ev('ArrowUp', { metaKey: true, altKey: true, shiftKey: true }), {}, true)).toBeNull()
    expect(homeRowStepForEvent(ev('ArrowLeft', { metaKey: true, altKey: true }), {}, true)).toBeNull()
  })

  it('off macOS: Ctrl+Alt, and ⌥⌘ (Super+Alt) does nothing', () => {
    const ev = (key: string, over: Partial<KeyboardEventInit>) => new KeyboardEvent('keydown', { key, code: key, ...over })
    expect(homeRowStepForEvent(ev('ArrowUp', { ctrlKey: true, altKey: true }), {}, false)).toBe(-1)
    expect(homeRowStepForEvent(ev('ArrowDown', { ctrlKey: true, altKey: true }), {}, false)).toBe(1)
    expect(homeRowStepForEvent(ev('ArrowDown', { metaKey: true, altKey: true }), {}, false)).toBeNull()
    // And on macOS, Ctrl+Alt is not it.
    expect(homeRowStepForEvent(ev('ArrowDown', { ctrlKey: true, altKey: true }), {}, true)).toBeNull()
  })

  it('a rebind wins over the default', () => {
    const kb = { homeRowDown: 'Meta+Alt+J' }
    const ev = (key: string, code: string) => new KeyboardEvent('keydown', { key, code, metaKey: true, altKey: true })
    expect(homeRowStepForEvent(ev('j', 'KeyJ'), kb, true)).toBe(1)
    expect(homeRowStepForEvent(ev('ArrowDown', 'ArrowDown'), kb, true)).toBeNull()
  })
})

describe('nextHomeRowIndex', () => {
  const all = (): boolean => true
  it('steps one row, stops at the ends, starts at an end with nothing selected', () => {
    expect(nextHomeRowIndex(4, 1, 1, all)).toBe(2)
    expect(nextHomeRowIndex(4, 1, -1, all)).toBe(0)
    expect(nextHomeRowIndex(4, 3, 1, all)).toBeNull()
    expect(nextHomeRowIndex(4, 0, -1, all)).toBeNull()
    expect(nextHomeRowIndex(4, null, 1, all)).toBe(0)
    expect(nextHomeRowIndex(4, null, -1, all)).toBe(3)
    expect(nextHomeRowIndex(0, null, 1, all)).toBeNull()
  })

  it('steps over rows that do not open', () => {
    expect(nextHomeRowIndex(4, 0, 1, (i) => i !== 1)).toBe(2)
    expect(nextHomeRowIndex(4, 3, -1, (i) => i === 0)).toBe(0)
    expect(nextHomeRowIndex(4, 0, 1, () => false)).toBeNull()
  })
})

describe('on Home', () => {
  it('↓ and ↑ move the selection in the order shown', () => {
    useHomeRoomsStore.setState({ shown: 'b::local' })
    press('ArrowDown')
    expect(shown()).toBe('c::local')
    press('ArrowDown')
    expect(shown()).toBe('d::local')
    press('ArrowUp')
    press('ArrowUp')
    press('ArrowUp')
    expect(opened()).toEqual(['c::local', 'd::local', 'c::local', 'b::local', 'a::local'])
  })

  it('follows the shown order: pins first, then most recent first', () => {
    act(() => {
      expect(useHomesStore.getState().pinRow(ids[0], 'c::local')).toBe('pinned')
      useHomesStore.getState().setRecentFirst(ids[0], true)
      useHomesStore.getState().stampRowsMessaged('d::local', 5000)
    })
    // Shown: c (pinned), d (most recent), a, b.
    for (let i = 0; i < 4; i++) press('ArrowDown')
    expect(opened()).toEqual(['c::local', 'd::local', 'a::local', 'b::local'])
  })

  it('stops at the ends (no wrap)', () => {
    useHomeRoomsStore.setState({ shown: 'd::local' })
    const down = press('ArrowDown')
    expect(opened()).toEqual([])
    expect(shown()).toBe('d::local')
    // Still taken: the keys never reach the terminal or the page.
    expect(down.defaultPrevented).toBe(true)
    useHomeRoomsStore.setState({ shown: 'a::local' })
    press('ArrowUp')
    expect(opened()).toEqual([])
  })

  it('with nothing selected, ↓ picks the first row and ↑ the last', () => {
    press('ArrowDown')
    expect(opened()).toEqual(['a::local'])
    useHomeRoomsStore.setState({ shown: null })
    press('ArrowUp')
    expect(opened()).toEqual(['a::local', 'd::local'])
  })

  it('a selection from another group counts as none', () => {
    useHomeRoomsStore.setState({ shown: 'x::local' })
    press('ArrowUp')
    expect(opened()).toEqual(['d::local'])
  })

  it('steps over a row whose click does nothing', () => {
    h.closed.add('b::local')
    useHomeRoomsStore.setState({ shown: 'a::local' })
    press('ArrowDown')
    expect(opened()).toEqual(['c::local'])
  })

  it('scrolls the row it opens into view', () => {
    const scrolled: string[] = []
    for (const a of ROWS) {
      const el = document.createElement('div')
      el.setAttribute('data-home-row', a)
      el.scrollIntoView = vi.fn(() => void scrolled.push(a))
      document.body.appendChild(el)
    }
    useHomeRoomsStore.setState({ shown: 'b::local' })
    press('ArrowDown')
    expect(scrolled).toEqual(['c::local'])
  })

  it('the collapsed rail: its icon scrolls into view', () => {
    const icon = document.createElement('button')
    icon.setAttribute('data-home-rail-row', 'a::local')
    icon.scrollIntoView = vi.fn()
    document.body.appendChild(icon)
    press('ArrowDown')
    expect(icon.scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' })
  })

  it('works from the compose box and keeps the keys from a terminal', () => {
    const compose = document.createElement('textarea')
    document.body.appendChild(compose)
    const term = document.createElement('textarea')
    const pty = vi.fn()
    term.addEventListener('keydown', pty)
    document.body.appendChild(term)
    useHomeRoomsStore.setState({ shown: 'a::local' })
    press('ArrowDown', { target: compose })
    press('ArrowDown', { target: term })
    expect(opened()).toEqual(['b::local', 'c::local'])
    expect(pty).not.toHaveBeenCalled()
  })

  it('a held key steps once', () => {
    press('ArrowDown')
    press('ArrowDown', { repeat: true })
    press('ArrowDown', { repeat: true })
    expect(opened()).toEqual(['a::local'])
  })

  it('off macOS: Ctrl+Alt+Up / Down step, ⌥⌘ does not', () => {
    h.mac = false
    useHomeRoomsStore.setState({ shown: 'b::local' })
    press('ArrowDown')
    press('ArrowDown', { mods: { metaKey: true, altKey: true } })
    expect(opened()).toEqual(['c::local'])
    press('ArrowUp')
    expect(opened()).toEqual(['c::local', 'b::local'])
  })
})

describe('where the keys stay with something else', () => {
  it('a code editor keeps them (CodeMirror: add a cursor above / below)', () => {
    const editor = document.createElement('div')
    editor.className = 'cm-editor'
    const content = document.createElement('div')
    content.setAttribute('contenteditable', 'true')
    editor.appendChild(content)
    const cm = vi.fn()
    content.addEventListener('keydown', cm)
    document.body.appendChild(editor)
    const ev = press('ArrowDown', { target: content })
    expect(opened()).toEqual([])
    expect(cm).toHaveBeenCalledTimes(1)
    expect(ev.defaultPrevented).toBe(false)
  })

  it('a dialog keeps them', () => {
    const dialog = document.createElement('div')
    dialog.setAttribute('role', 'dialog')
    const input = document.createElement('input')
    dialog.appendChild(input)
    document.body.appendChild(dialog)
    press('ArrowDown', { target: input })
    expect(opened()).toEqual([])
  })

  it('off Home, with Settings open, they do nothing', () => {
    usePageViewStore.setState({ page: 'agents' })
    const ev = press('ArrowDown')
    expect(ev.defaultPrevented).toBe(false)
    usePageViewStore.setState({ page: 'home' })
    useSettingsStore.setState({ settingsOpen: true })
    press('ArrowDown')
    expect(opened()).toEqual([])
  })

  it('⌥⌘← / ⌥⌘→ (previous / next tab) are left alone', () => {
    const left = new KeyboardEvent('keydown', { key: 'ArrowLeft', code: 'ArrowLeft', metaKey: true, altKey: true, bubbles: true, cancelable: true })
    act(() => {
      window.dispatchEvent(left)
    })
    expect(left.defaultPrevented).toBe(false)
    expect(opened()).toEqual([])
  })
})
