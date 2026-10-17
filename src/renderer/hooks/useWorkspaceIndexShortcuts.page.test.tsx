// @vitest-environment jsdom
// Page switch chords (0.46.0, Rosson 2026-10-16): ⌃⌘1 My Home · ⌃⌘2 Agents
// · ⌃⌘3 Projects · ⌃⌘4 Tickets on macOS, Ctrl+Alt+1–4 on Linux / Windows,
// rebindable. ⌘1–9, ⌥⌘1–9 and ⌥⌘0 keep doing what they did.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  mac: true,
  zen: false,
  switchGarden: vi.fn(),
  openHomeRow: vi.fn(),
  zenSelectRow: vi.fn(),
  active: [] as unknown[],
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => h.mac,
}))
vi.mock('@/lib/zen/zen-view', async (orig) => ({
  ...(await orig<typeof import('@/lib/zen/zen-view')>()),
  zenShownNow: () => h.zen,
  zenSelectRow: h.zenSelectRow,
}))
vi.mock('@/lib/zen/zen-gardens', async (orig) => ({
  ...(await orig<typeof import('@/lib/zen/zen-gardens')>()),
  switchZenGardenByIndex: h.switchGarden,
}))
vi.mock('@/lib/home-open', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-open')>()),
  openHomeRow: h.openHomeRow,
}))
vi.mock('@/components/Home/home-room', async (orig) => ({
  ...(await orig<typeof import('@/components/Home/home-room')>()),
  homeRowOpenableNow: () => true,
}))
vi.mock('@/components/Sidebar/ActiveBar', async (orig) => ({
  ...(await orig<typeof import('@/components/Sidebar/ActiveBar')>()),
  getActiveBarItems: () => h.active,
}))

import { useWorkspaceIndexShortcuts } from './useWorkspaceIndexShortcuts'
import { registerZenViewSwitch } from '@/lib/zen/zen-view'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useHomesStore } from '@/stores/homes'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { PAGE_SHORTCUTS, keyEventMatchesCombo, pageComboLabel, pageShortcutsSummary } from '@/lib/page-shortcuts'
import { pageTabTitle } from '@/components/TopBar/PageTabs'

interface Mods {
  meta?: boolean
  ctrl?: boolean
  alt?: boolean
  shift?: boolean
  altGraph?: boolean
  key?: string
}

function press(code: string, m: Mods = {}): KeyboardEvent {
  const digit = /^Digit(\d)$/.exec(code)
  const e = new KeyboardEvent('keydown', {
    key: m.key ?? (digit ? digit[1] : code.replace(/^Key/, '').toLowerCase()),
    code,
    metaKey: !!m.meta,
    ctrlKey: !!m.ctrl,
    altKey: !!m.alt,
    shiftKey: !!m.shift,
    bubbles: true,
    cancelable: true,
  })
  if (m.altGraph) Object.defineProperty(e, 'getModifierState', { value: (k: string) => k === 'AltGraph' })
  window.dispatchEvent(e)
  return e
}

const macChord = (n: number): KeyboardEvent => press(`Digit${n}`, { ctrl: true, meta: true })
const otherChord = (n: number): KeyboardEvent => press(`Digit${n}`, { ctrl: true, alt: true })

const ORDER: [number, AppPage][] = [
  [1, 'home'],
  [2, 'agents'],
  [3, 'projects'],
  [4, 'feedback'],
]

let setActiveWorkspace: ReturnType<typeof vi.fn>
let selectHome: ReturnType<typeof vi.fn>

function project(i: number): Record<string, unknown> {
  return {
    id: `p${i}`,
    name: `p${i}`,
    path: `/ws/p${i}`,
    agentMode: 'agent',
    pinned: false,
    worktreeMode: 0,
    workspaces: [{ id: `w${i}` }],
  }
}

beforeEach(() => {
  h.mac = true
  h.zen = false
  h.switchGarden.mockClear()
  h.openHomeRow.mockClear()
  h.zenSelectRow.mockClear()
  setActiveWorkspace = vi.fn()
  selectHome = vi.fn()
  const projects = Array.from({ length: 10 }, (_, i) => project(i + 1))
  h.active = projects
  useProjectsStore.setState({ projects, setActiveWorkspace } as never)
  useHomesStore.setState({
    homes: [
      { id: 'h1', name: 'One', rows: [{ address: 'a::local', workspaceId: 'p1', label: 'A' }] },
      { id: 'h2', name: 'Two', rows: [] },
    ],
    selectedId: 'h1',
    selectHome,
  } as never)
  useFocusGroupsStore.setState({ focusGroupsEnabled: false } as never)
  // ⌘N = pinned, ⌥⌘N = active.
  useTerminalSettingsStore.setState({ shortcutLayout: 'cmd-pinned-cmdshift-active' } as never)
  useSettingsStore.setState({ keybindings: {}, settingsOpen: false })
  usePageViewStore.getState().setPage('wiki')
  renderHook(() => useWorkspaceIndexShortcuts())
})

afterEach(() => {
  cleanup()
})

describe('page switch chords', () => {
  it.each(ORDER)('macOS: ⌃⌘%i shows %s', (n, page) => {
    const e = macChord(n)
    expect(usePageViewStore.getState().page).toBe(page)
    expect(e.defaultPrevented).toBe(true)
    // A page chord never also picks a row.
    expect(setActiveWorkspace).not.toHaveBeenCalled()
    expect(h.openHomeRow).not.toHaveBeenCalled()
  })

  it.each(ORDER)('Linux / Windows: Ctrl+Alt+%i shows %s', (n, page) => {
    h.mac = false
    const e = otherChord(n)
    expect(usePageViewStore.getState().page).toBe(page)
    expect(e.defaultPrevented).toBe(true)
    expect(setActiveWorkspace).not.toHaveBeenCalled()
  })

  it('each platform answers only its own chord', () => {
    otherChord(2)
    expect(usePageViewStore.getState().page).toBe('wiki')
    h.mac = false
    macChord(2)
    expect(usePageViewStore.getState().page).toBe('wiki')
    expect(setActiveWorkspace).not.toHaveBeenCalled()
  })

  it('AltGr (Ctrl+Alt on Windows) still types its character', () => {
    h.mac = false
    const e = press('Digit3', { ctrl: true, alt: true, altGraph: true, key: '#' })
    expect(usePageViewStore.getState().page).toBe('wiki')
    expect(e.defaultPrevented).toBe(false)
  })

  it('reads the digit from e.code (AZERTY, Option)', () => {
    press('Digit3', { ctrl: true, meta: true, key: '"' })
    expect(usePageViewStore.getState().page).toBe('projects')
  })

  it('Shift or a missing modifier is not the chord; ⌃⌘5–9 pick nothing', () => {
    press('Digit1', { ctrl: true, meta: true, shift: true })
    press('Digit1', { ctrl: true })
    for (const n of [5, 6, 7, 8, 9, 0]) macChord(n)
    expect(usePageViewStore.getState().page).toBe('wiki')
    expect(setActiveWorkspace).not.toHaveBeenCalled()
  })

  it('closes Settings first, like a click on the tab', () => {
    useSettingsStore.setState({ settingsOpen: true })
    macChord(3)
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
    expect(usePageViewStore.getState().page).toBe('projects')
  })

  it('a key repeat does not act again', () => {
    usePageViewStore.getState().setPage('agents')
    window.dispatchEvent(
      new KeyboardEvent('keydown', { key: '4', code: 'Digit4', ctrlKey: true, metaKey: true, repeat: true, cancelable: true }),
    )
    expect(usePageViewStore.getState().page).toBe('agents')
  })

  it('rebindable: a custom binding replaces the default', () => {
    useSettingsStore.setState({ keybindings: { goToHome: 'Ctrl+Meta+H' } })
    macChord(1)
    expect(usePageViewStore.getState().page).toBe('wiki')
    press('KeyH', { ctrl: true, meta: true })
    expect(usePageViewStore.getState().page).toBe('home')
    // The others keep their defaults.
    macChord(4)
    expect(usePageViewStore.getState().page).toBe('feedback')
  })

  it('in Zen: the rail view, never the page under Zen', () => {
    h.zen = true
    const views: string[] = []
    const off = registerZenViewSwitch((v) => views.push(v))
    for (const n of [1, 2, 3, 4]) macChord(n)
    off()
    expect(views).toEqual(['home', 'agents', 'projects', 'tickets'])
    expect(usePageViewStore.getState().page).toBe('wiki')
    expect(h.zenSelectRow).not.toHaveBeenCalled()
  })
})

describe('the digit chords already there are unchanged', () => {
  it('⌘1–9 pick a pinned row on Agents', () => {
    usePageViewStore.getState().setPage('agents')
    press('Digit3', { meta: true })
    expect(setActiveWorkspace).toHaveBeenCalledWith('p3', 'w3')
    expect(usePageViewStore.getState().page).toBe('agents')
  })

  it('⌘1 opens Home row 1 on Home', () => {
    usePageViewStore.getState().setPage('home')
    press('Digit1', { meta: true })
    expect(h.openHomeRow).toHaveBeenCalledTimes(1)
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('⌥⌘2 switches to Home 2 on Home', () => {
    usePageViewStore.getState().setPage('home')
    press('Digit2', { meta: true, alt: true, key: '™' })
    expect(selectHome).toHaveBeenCalledWith('h2')
    expect(setActiveWorkspace).not.toHaveBeenCalled()
  })

  it('⌥⌘1–9 and ⌥⌘0 pick an active row on Agents (⌥⌘0 the tenth)', () => {
    usePageViewStore.getState().setPage('agents')
    press('Digit2', { meta: true, alt: true, key: '™' })
    expect(setActiveWorkspace).toHaveBeenLastCalledWith('p2', 'w2')
    press('Digit0', { meta: true, alt: true, key: 'º' })
    expect(setActiveWorkspace).toHaveBeenLastCalledWith('p10', 'w10')
    expect(usePageViewStore.getState().page).toBe('agents')
  })

  it('in Zen: ⌥⌘N switches Garden N and ⌘N opens conversation N', () => {
    h.zen = true
    press('Digit2', { meta: true, alt: true, key: '™' })
    expect(h.switchGarden).toHaveBeenCalledWith(1)
    press('Digit3', { meta: true })
    expect(h.zenSelectRow).toHaveBeenCalledWith(2)
  })
})

describe('labels', () => {
  it('the defaults per platform, in top-bar order', () => {
    expect(PAGE_SHORTCUTS.map((s) => s.page)).toEqual(['home', 'agents', 'projects', 'feedback'])
    expect(pageShortcutsSummary({}, true)).toBe('⌃⌘1–4')
    expect(pageShortcutsSummary({}, false)).toBe('Ctrl+Alt+1–4')
    expect(pageShortcutsSummary({ goToAgents: 'Ctrl+Meta+A' }, true)).toBe('⌃⌘1 / ⌃⌘A / ⌃⌘3 / ⌃⌘4')
    expect(pageComboLabel('Ctrl+Meta+2', true)).toBe('⌃⌘2')
    expect(pageComboLabel('Ctrl+Alt+2', false)).toBe('Ctrl+Alt+2')
  })

  it('the page tab tooltips carry the chord; Projects keeps ⌘P', () => {
    expect(pageTabTitle('home', {}, true)).toMatch(/\(⌃⌘1\)$/)
    expect(pageTabTitle('agents', {}, false)).toMatch(/\(Ctrl\+Alt\+2\)$/)
    expect(pageTabTitle('projects', {}, true)).toMatch(/\(⌃⌘3, ⌘P\)$/)
    expect(pageTabTitle('feedback', {}, true)).toMatch(/\(⌃⌘4\)$/)
    expect(pageTabTitle('wiki', {}, true)).toBe('Wiki')
  })

  it('combo matching is exact on modifiers', () => {
    const e = { key: '1', code: 'Digit1', metaKey: true, ctrlKey: true, altKey: false, shiftKey: false }
    expect(keyEventMatchesCombo(e, 'Ctrl+Meta+1')).toBe(true)
    expect(keyEventMatchesCombo(e, 'Meta+1')).toBe(false)
    expect(keyEventMatchesCombo({ ...e, altKey: true }, 'Ctrl+Meta+1')).toBe(false)
    expect(keyEventMatchesCombo(e, '')).toBe(false)
  })
})
