// @vitest-environment jsdom
// Page switch chords (0.46.0, Rosson 2026-10-16): ⌃⌘1 My Home · ⌃⌘2 Agents
// · ⌃⌘3 Projects · ⌃⌘4 Tickets on macOS, Ctrl+Alt+1–4 on Linux / Windows,
// rebindable. 0.46.4 (Rosson 2026-10-24): ⌃⌘5 Wiki (the Wiki tab's open,
// leaving Zen) and ⌃⌘6 Settings (⌘,'s open, over Zen). ⌘1–9, ⌥⌘1–9 and
// ⌥⌘0 keep doing what they did.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  mac: true,
  zen: false,
  exitZen: vi.fn(() => {
    h.zen = false
  }),
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
  zenOnNow: () => h.zen,
  exitZen: h.exitZen,
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
import {
  PAGE_SHORTCUTS,
  keyEventMatchesCombo,
  pageComboLabel,
  pageShortcutKeys,
  pageShortcutsSummary,
} from '@/lib/page-shortcuts'
import { pageTabTitle, settingsTabTitle } from '@/components/TopBar/PageTabs'
import { getDefaultKeybindings } from '@shared/hotkeys'

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
  h.exitZen.mockClear()
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

  it('Shift or a missing modifier is not the chord; ⌃⌘7–9 and ⌃⌘0 pick nothing', () => {
    press('Digit1', { ctrl: true, meta: true, shift: true })
    press('Digit1', { ctrl: true })
    press('Digit5', { ctrl: true, meta: true, shift: true })
    press('Digit6', { ctrl: true, alt: true })
    for (const n of [7, 8, 9, 0]) macChord(n)
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

describe('⌃⌘5 Wiki and ⌃⌘6 Settings (0.46.4)', () => {
  it('macOS: ⌃⌘5 opens the wiki as the Wiki tab does (All of K2, the page to return to)', () => {
    usePageViewStore.getState().setPage('projects')
    const e = macChord(5)
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiReturnPage, s.wikiOpenAll]).toEqual(['wiki', 'projects', true])
    expect(e.defaultPrevented).toBe(true)
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
    expect(setActiveWorkspace).not.toHaveBeenCalled()
    expect(h.openHomeRow).not.toHaveBeenCalled()
  })

  it('Linux / Windows: Ctrl+Alt+5 opens the wiki, Ctrl+Alt+6 Settings', () => {
    h.mac = false
    usePageViewStore.getState().setPage('agents')
    const wiki = otherChord(5)
    expect(usePageViewStore.getState().page).toBe('wiki')
    expect(wiki.defaultPrevented).toBe(true)
    const settings = otherChord(6)
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
    expect(settings.defaultPrevented).toBe(true)
    expect(setActiveWorkspace).not.toHaveBeenCalled()
  })

  it('already on the wiki, ⌃⌘5 keeps the wiki it shows', () => {
    usePageViewStore.getState().setPage('home')
    usePageViewStore.getState().openWiki('/ws/p3')
    macChord(5)
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.wikiReturnPage, s.wikiOpenAll]).toEqual(['wiki', '/ws/p3', 'home', false])
  })

  it('⌃⌘5 from Settings closes Settings and opens the wiki', () => {
    usePageViewStore.getState().setPage('feedback')
    useSettingsStore.setState({ settingsOpen: true })
    macChord(5)
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
    expect(usePageViewStore.getState().page).toBe('wiki')
  })

  it('⌃⌘6 opens Settings as ⌘, does (the page under it stays); again, it stays open', () => {
    usePageViewStore.getState().setPage('projects')
    const e = macChord(6)
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
    expect(e.defaultPrevented).toBe(true)
    expect(usePageViewStore.getState().page).toBe('projects')
    useSettingsStore.getState().setSection('keybindings')
    macChord(6)
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
    expect(useSettingsStore.getState().activeSection).toBe('keybindings')
  })

  it('AltGr+5 / AltGr+6 (Ctrl+Alt on Windows) still type their character', () => {
    h.mac = false
    usePageViewStore.getState().setPage('agents')
    const five = press('Digit5', { ctrl: true, alt: true, altGraph: true, key: '[' })
    const six = press('Digit6', { ctrl: true, alt: true, altGraph: true, key: '|' })
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
    expect(five.defaultPrevented).toBe(false)
    expect(six.defaultPrevented).toBe(false)
  })

  it('rebindable: custom bindings replace ⌃⌘5 and ⌃⌘6', () => {
    usePageViewStore.getState().setPage('agents')
    useSettingsStore.setState({ keybindings: { goToWiki: 'Ctrl+Meta+W', goToSettings: 'Ctrl+Meta+S' } })
    macChord(5)
    macChord(6)
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(useSettingsStore.getState().settingsOpen).toBe(false)
    press('KeyW', { ctrl: true, meta: true })
    expect(usePageViewStore.getState().page).toBe('wiki')
    press('KeyS', { ctrl: true, meta: true })
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
  })

  it('in Zen: ⌃⌘5 leaves Zen and opens the wiki; no rail view', () => {
    h.zen = true
    usePageViewStore.getState().setPage('home')
    const views: string[] = []
    const off = registerZenViewSwitch((v) => views.push(v))
    macChord(5)
    off()
    expect(h.exitZen).toHaveBeenCalledTimes(1)
    expect(usePageViewStore.getState().page).toBe('wiki')
    expect(views).toEqual([])
  })

  it('in Zen with the wiki already under it: ⌃⌘5 still leaves Zen', () => {
    h.zen = true
    macChord(5)
    expect(h.exitZen).toHaveBeenCalledTimes(1)
    expect(usePageViewStore.getState().page).toBe('wiki')
  })

  it('in Zen: ⌃⌘6 opens Settings over Zen, as ⌘, does; Zen stays on, the page stays', () => {
    h.zen = true
    usePageViewStore.getState().setPage('home')
    const views: string[] = []
    const off = registerZenViewSwitch((v) => views.push(v))
    macChord(6)
    off()
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
    expect(h.exitZen).not.toHaveBeenCalled()
    expect(h.zen).toBe(true)
    expect(usePageViewStore.getState().page).toBe('home')
    expect(views).toEqual([])
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
    expect(PAGE_SHORTCUTS.map((s) => s.target)).toEqual(['home', 'agents', 'projects', 'feedback', 'wiki', 'settings'])
    expect(PAGE_SHORTCUTS.map((s) => s.zenView)).toEqual(['home', 'agents', 'projects', 'tickets', null, null])
    expect(pageShortcutsSummary({}, true)).toBe('⌃⌘1–4')
    expect(pageShortcutsSummary({}, false)).toBe('Ctrl+Alt+1–4')
    expect(pageShortcutsSummary({ goToAgents: 'Ctrl+Meta+A' }, true)).toBe('⌃⌘1 / ⌃⌘A / ⌃⌘3 / ⌃⌘4')
    expect(pageComboLabel('Ctrl+Meta+2', true)).toBe('⌃⌘2')
    expect(pageComboLabel('Ctrl+Alt+2', false)).toBe('Ctrl+Alt+2')
    // The cheat sheet: rail views, Wiki, Settings.
    expect(pageShortcutKeys({}, true)).toEqual({ views: '⌃⌘1–4', wiki: '⌃⌘5', settings: '⌃⌘6' })
    expect(pageShortcutKeys({}, false)).toEqual({ views: 'Ctrl+Alt+1–4', wiki: 'Ctrl+Alt+5', settings: 'Ctrl+Alt+6' })
    // Rebinding Wiki or Settings never breaks the rail views' summary.
    expect(pageShortcutKeys({ goToWiki: 'Ctrl+Meta+W' }, true)).toEqual({ views: '⌃⌘1–4', wiki: '⌃⌘W', settings: '⌃⌘6' })
  })

  it('no other default binding uses ⌃⌘5/6 or Ctrl+Alt+5/6', () => {
    for (const mac of [true, false]) {
      const defaults = getDefaultKeybindings(mac)
      const chords = mac ? ['Ctrl+Meta+5', 'Ctrl+Meta+6'] : ['Ctrl+Alt+5', 'Ctrl+Alt+6']
      for (const combo of chords) {
        expect(Object.entries(defaults).filter(([, c]) => c === combo).map(([id]) => id)).toEqual([
          combo.endsWith('5') ? 'goToWiki' : 'goToSettings',
        ])
      }
    }
  })

  it('the page tab tooltips carry the chord; Projects keeps ⌘P', () => {
    expect(pageTabTitle('home', {}, true)).toMatch(/\(⌃⌘1\)$/)
    expect(pageTabTitle('agents', {}, false)).toMatch(/\(Ctrl\+Alt\+2\)$/)
    expect(pageTabTitle('projects', {}, true)).toMatch(/\(⌃⌘3, ⌘P\)$/)
    expect(pageTabTitle('feedback', {}, true)).toMatch(/\(⌃⌘4\)$/)
    expect(pageTabTitle('wiki', {}, true)).toBe('Wiki (⌃⌘5)')
    expect(pageTabTitle('wiki', {}, false)).toBe('Wiki (Ctrl+Alt+5)')
    expect(pageTabTitle('wiki', { goToWiki: 'Ctrl+Meta+W' }, true)).toBe('Wiki (⌃⌘W)')
    expect(settingsTabTitle({}, 'mac')).toBe('Settings (⌃⌘6, ⌘,)')
    expect(settingsTabTitle({}, 'linux')).toBe('Settings (Ctrl+Alt+6, Super+,)')
    expect(settingsTabTitle({}, 'windows')).toBe('Settings (Ctrl+Alt+6)')
    expect(settingsTabTitle({ goToSettings: 'Ctrl+Meta+S' }, 'mac')).toBe('Settings (⌃⌘S, ⌘,)')
  })

  it('combo matching is exact on modifiers', () => {
    const e = { key: '1', code: 'Digit1', metaKey: true, ctrlKey: true, altKey: false, shiftKey: false }
    expect(keyEventMatchesCombo(e, 'Ctrl+Meta+1')).toBe(true)
    expect(keyEventMatchesCombo(e, 'Meta+1')).toBe(false)
    expect(keyEventMatchesCombo({ ...e, altKey: true }, 'Ctrl+Meta+1')).toBe(false)
    expect(keyEventMatchesCombo(e, '')).toBe(false)
  })
})
