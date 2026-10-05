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

const h = vi.hoisted(() => ({ label: 'main' }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
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
    if (c.hasAttribute('data-zen-enter')) return 'zen'
    if (c.hasAttribute('data-stub')) return c.getAttribute('data-stub') ?? '?'
    if (c.hasAttribute('data-drawer')) return c.getAttribute('data-drawer') ?? '?'
    return 'pipe'
  })
}

beforeEach(() => {
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
    expect(toggle?.getAttribute('title')).toBe('Zen Mode (⌃⌘Z). Hold Shift for safe mode.')
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
      'Zen Mode (Ctrl+Alt+Z). Hold Shift for safe mode.',
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

// Rosson 2026-10-04: the top bar's Zen toggle draws the ensō (picked from
// three candidates; `lib/zen/zen-icon.ts`). Pointing at it draws the
// stroke in again, unless the OS asks for reduced motion.
describe('the ensō icon on the top bar’s Zen toggle', () => {
  let restoreMatchMedia: PropertyDescriptor | undefined
  let reduced = false

  beforeEach(() => {
    reduced = false
    restoreMatchMedia = Object.getOwnPropertyDescriptor(window, 'matchMedia')
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      value: (query: string) => ({
        matches: query === '(prefers-reduced-motion: reduce)' ? reduced : false,
        media: query,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      }),
    })
  })

  afterEach(() => {
    if (restoreMatchMedia) Object.defineProperty(window, 'matchMedia', restoreMatchMedia)
    else delete (window as { matchMedia?: unknown }).matchMedia
  })

  function icon(): SVGSVGElement {
    const el = document.querySelector('[data-zen-enter] svg[data-zen-icon]')
    if (!(el instanceof SVGSVGElement)) throw new Error('no Zen icon in the top-bar toggle')
    return el
  }

  it('one toggle, the ensō in its off state, with a pointer', () => {
    render(<TopBarUtilities />)
    const toggles = Array.from(document.querySelectorAll('[data-zen-enter]'))
    expect(toggles.length).toBe(1)
    expect(toggles[0].classList.contains('cursor-pointer')).toBe(true)
    expect([icon().getAttribute('data-zen-icon'), icon().getAttribute('data-zen-icon-on')]).toEqual(['enso', 'false'])
    expect([icon().getAttribute('width'), icon().getAttribute('viewBox')]).toEqual(['16', '0 0 24 24'])
    // Still until pointed at.
    expect(icon().getAttribute('data-zen-icon-motion')).toBe('still')
    expect(document.querySelector('[data-zen-enter] mask')).toBeNull()
  })

  it('hover draws the ensō in again', () => {
    render(<TopBarUtilities />)
    act(() => void fireEvent.mouseEnter(document.querySelector('[data-zen-enter]') as HTMLElement))
    expect(icon().getAttribute('data-zen-icon-motion')).toBe('play')
    const mask = document.querySelector('[data-zen-enter] mask')
    if (!mask) throw new Error('no ensō draw mask on hover')
    expect(document.querySelector('[data-zen-enter] [data-zen-icon-part="enso"]')?.getAttribute('mask')).toBe(`url(#${mask.id})`)
  })

  it('reduced motion: hover leaves it still', () => {
    reduced = true
    render(<TopBarUtilities />)
    act(() => void fireEvent.mouseEnter(document.querySelector('[data-zen-enter]') as HTMLElement))
    act(() => void fireEvent.mouseEnter(document.querySelector('[data-zen-enter]') as HTMLElement))
    expect(icon().getAttribute('data-zen-icon-motion')).toBe('still')
    expect(document.querySelector('[data-zen-enter] mask')).toBeNull()
  })
})
