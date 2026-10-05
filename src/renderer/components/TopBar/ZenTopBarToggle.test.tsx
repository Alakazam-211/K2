// @vitest-environment jsdom
//
// prd-zen-gardens-v1 G5 / G63 (TG3.4) — the way into Zen is one square
// toggle, the LAST item of the shared top-bar cluster: right of the two
// drawer toggles on Agents and Home (after a pipe), after the mode toggle
// on pages without drawer toggles (Settings, Feedback). It turns THIS
// window's Zen on (Shift: safe mode) and is absent where Zen doesn't exist:
// Windows until G-Win, and Focus / ticket windows. (The web client is
// `components/Zen/zen-web.test.tsx`.)

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  label: 'main',
  // The Zen toggle's icon config (TEMPORARY preview): null runs the real
  // constants of `lib/zen/zen-icon.ts`; set to force preview on or off.
  icon: null as null | { preview: boolean; choice: 'ripples' | 'enso' | 'bonsai' },
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/zen/zen-icon', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/lib/zen/zen-icon')>()
  return {
    ...real,
    zenToggleIcons: () => (h.icon ? real.zenToggleIcons(h.icon.preview, h.icon.choice) : real.zenToggleIcons()),
  }
})
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: h.label }) }))
// The cluster's own buttons are not what this file checks.
vi.mock('@/components/Timer/TimerButton', () => ({ default: () => <span data-stub="timer" /> }))
vi.mock('@/components/Timer/UsageButton', () => ({ default: () => <span data-stub="usage" /> }))
vi.mock('@/components/Timer/KeepAwakeButton', () => ({ default: () => <span data-stub="keep-awake" /> }))
vi.mock('@/components/CheatSheet/K2NounsCheatSheet', () => ({ default: () => <span data-stub="cheat-sheet" /> }))
vi.mock('@/components/Presence/PresenceRoster', () => ({ default: () => <span data-stub="presence" /> }))
vi.mock('@/components/Presence/ModeToggle', () => ({ default: () => <span data-stub="mode" /> }))

import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import TopBarUtilities from './TopBarUtilities'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { useZenViewStore } from '@/lib/zen/zen-view'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { zenToggleIcons } from '@/lib/zen/zen-icon'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function setWindow(platform: string, label = 'main'): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  h.label = label
  __resetZenAvailableForTests()
}

/** The cluster's items, in order, by what they are. */
function clusterItems(): string[] {
  const cluster = document.querySelector('.no-drag.flex.items-center.gap-1')
  if (!cluster) throw new Error('no top-bar cluster')
  return Array.from(cluster.children).map((c) => {
    // One toggle, or (the TEMPORARY icon preview) one group of three.
    if (c.hasAttribute('data-zen-enter') || c.hasAttribute('data-zen-icon-preview')) return 'zen'
    if (c.hasAttribute('data-stub')) return c.getAttribute('data-stub') ?? '?'
    if (c.hasAttribute('data-drawer')) return c.getAttribute('data-drawer') ?? '?'
    return 'pipe'
  })
}

/** While the TEMPORARY icon preview is on, the first toggle's tooltip
 *  names its option first. */
function titlePrefix(): string {
  const icons = zenToggleIcons()
  return icons.length > 1 ? `${icons[0].label}. ` : ''
}

beforeEach(() => {
  h.icon = null
  setWindow('MacIntel')
  useZenWindowStore.setState({ on: false, garden: null })
  useZenViewStore.setState({ safe: null, epoch: 0 })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('agents')
})

afterEach(() => cleanup())

describe('the Zen toggle in the top bar (G5, TG3.4)', () => {
  it('Agents / Home: the last item, right of both drawer toggles, after a pipe', () => {
    render(
      <TopBarUtilities>
        <button data-drawer="left" />
        <button data-drawer="right" />
      </TopBarUtilities>,
    )
    expect(clusterItems()).toEqual([
      'presence',
      'usage',
      'pipe',
      'timer',
      'keep-awake',
      'cheat-sheet',
      'pipe',
      'mode',
      'pipe',
      'left',
      'right',
      'pipe',
      'zen',
    ])
    const toggle = document.querySelector('[data-zen-enter]')
    expect(toggle?.getAttribute('aria-pressed')).toBe('false')
    expect(toggle?.getAttribute('title')).toBe(`${titlePrefix()}Zen Mode (⌃⌘Z). Hold Shift for safe mode.`)
  })

  it('a page without drawer toggles (Settings, Feedback): after the mode toggle, with its own pipe', () => {
    render(<TopBarUtilities />)
    expect(clusterItems().slice(-3)).toEqual(['mode', 'pipe', 'zen'])
  })

  it('a click turns this window’s Zen on over the current page; Shift starts safe mode', () => {
    render(<TopBarUtilities />)
    // Rosson 2026-10-04: a pointer on hover.
    expect(document.querySelector('[data-zen-enter]')?.classList.contains('cursor-pointer')).toBe(true)
    act(() => void fireEvent.click(document.querySelector('[data-zen-enter]') as HTMLElement))
    expect(useZenWindowStore.getState().on).toBe(true)
    expect(useZenViewStore.getState().safe).toBeNull()
    expect(usePageViewStore.getState().page).toBe('agents')
    act(() => useZenWindowStore.getState().setOn(false))
    act(() => void fireEvent.click(document.querySelector('[data-zen-enter]') as HTMLElement, { shiftKey: true }))
    expect(useZenWindowStore.getState().on).toBe(true)
    expect(useZenViewStore.getState().safe).toEqual({ kind: 'shift' })
  })

  it('Linux shows the Linux chord', () => {
    setWindow('Linux x86_64')
    render(<TopBarUtilities />)
    expect(document.querySelector('[data-zen-enter]')?.getAttribute('title')).toBe(
      `${titlePrefix()}Zen Mode (Ctrl+Alt+Z). Hold Shift for safe mode.`,
    )
  })

  it('absent where Zen doesn’t exist: Windows (G-Win), Focus and ticket windows — and no stray pipe', () => {
    for (const [platform, label] of [
      ['Win32', 'main'],
      ['MacIntel', 'focus-abc'],
      ['MacIntel', 'window-ticket-42'],
    ] as const) {
      setWindow(platform, label)
      render(
        <TopBarUtilities>
          <button data-drawer="left" />
        </TopBarUtilities>,
      )
      expect([platform, label, document.querySelector('[data-zen-enter]')]).toEqual([platform, label, null])
      expect(clusterItems().slice(-1)).toEqual(['left'])
      cleanup()
    }
  })
})

// Rosson 2026-10-04: picking the Zen toggle's icon (`lib/zen/zen-icon.ts`).
describe('the Zen icon preview in the top bar (TEMPORARY)', () => {
  it('preview on: three real Zen toggles side by side in one marked group, ripples, enso, bonsai', () => {
    h.icon = { preview: true, choice: 'ripples' }
    render(<TopBarUtilities />)
    expect(clusterItems().slice(-3)).toEqual(['mode', 'pipe', 'zen'])
    const group = document.querySelector('[data-zen-icon-preview]')
    if (!(group instanceof HTMLElement)) throw new Error('no icon preview group')
    expect(group.getAttribute('aria-label')).toBe('Zen icon preview (temporary): pick one')
    expect(group.classList.contains('border-dashed')).toBe(true)
    const toggles = Array.from(group.querySelectorAll('[data-zen-enter]'))
    expect(
      toggles.map((t) => [t.getAttribute('data-zen-icon-option'), t.querySelector('svg')?.getAttribute('data-zen-icon'), t.getAttribute('title')]),
    ).toEqual([
      ['ripples', 'ripples', 'Option 1: Stone & ripples. Zen Mode (⌃⌘Z). Hold Shift for safe mode.'],
      ['enso', 'enso', 'Option 2: Ensō. Zen Mode (⌃⌘Z). Hold Shift for safe mode.'],
      ['bonsai', 'bonsai', 'Option 3: Bonsai. Zen Mode (⌃⌘Z). Hold Shift for safe mode.'],
    ])
    // Outside Zen every icon is in its off state.
    expect(toggles.map((t) => t.querySelector('svg')?.getAttribute('data-zen-icon-on'))).toEqual(['false', 'false', 'false'])
    expect(toggles.every((t) => t.classList.contains('cursor-pointer'))).toBe(true)
  })

  it('preview on: each one turns this window’s Zen on (Shift: safe mode)', () => {
    h.icon = { preview: true, choice: 'ripples' }
    render(<TopBarUtilities />)
    for (const variant of ['ripples', 'enso', 'bonsai']) {
      act(() => useZenWindowStore.getState().setOn(false))
      act(() => useZenViewStore.setState({ safe: null }))
      const toggle = document.querySelector(`[data-zen-enter][data-zen-icon-option="${variant}"]`)
      if (!(toggle instanceof HTMLElement)) throw new Error(`no ${variant} toggle`)
      act(() => void fireEvent.click(toggle))
      expect([variant, useZenWindowStore.getState().on, useZenViewStore.getState().safe]).toEqual([variant, true, null])
      act(() => useZenWindowStore.getState().setOn(false))
      act(() => void fireEvent.click(toggle, { shiftKey: true }))
      expect([variant, useZenWindowStore.getState().on, useZenViewStore.getState().safe]).toEqual([variant, true, { kind: 'shift' }])
    }
    expect(usePageViewStore.getState().page).toBe('agents')
  })

  it.each(['ripples', 'enso', 'bonsai'] as const)('preview off: one toggle with the chosen icon (%s)', (choice) => {
    h.icon = { preview: false, choice }
    render(<TopBarUtilities />)
    expect(document.querySelector('[data-zen-icon-preview]')).toBeNull()
    expect(clusterItems().slice(-3)).toEqual(['mode', 'pipe', 'zen'])
    const toggles = Array.from(document.querySelectorAll('[data-zen-enter]'))
    expect(toggles.map((t) => [t.querySelector('svg')?.getAttribute('data-zen-icon'), t.getAttribute('title')])).toEqual([
      [choice, 'Zen Mode (⌃⌘Z). Hold Shift for safe mode.'],
    ])
    act(() => void fireEvent.click(toggles[0]))
    expect(useZenWindowStore.getState().on).toBe(true)
  })
})
