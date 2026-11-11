// @vitest-environment jsdom
//
// The sidebar moves to the setup list's floor as the list opens, and back to
// the saved width as it folds or setup finishes, on the slow motion tokens.
// Never at boot, on a drag, when the floor doesn't change the width, or
// under reduced motion.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ freeze: vi.fn(), release: vi.fn() }))

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => new Promise(() => {})),
  settingsUpdate: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(() => new Promise(() => {})),
  daemonCliPost: vi.fn(() => new Promise(() => {})),
}))
vi.mock('@/lib/browser-overlay', () => ({
  beginBrowserMotion: () => {
    h.freeze()
    return h.release
  },
}))

import { act, useRef } from 'react'
import { cleanup, render } from '@testing-library/react'
import { SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH } from '../../../shared/constants'
import { primaryScope } from '@/kessel/server-scope'
import { parseFirstRunDoc, resetFirstRunForTests, useFirstRunStore } from '@/stores/first-run'
import { setSidebarResizing, useSidebarDisplayWidth, useSidebarStore } from '@/stores/sidebar'
import { useSidebarFloorMotion } from './sidebar-floor-motion'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const FLOOR = SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH

function setDoc(over: Record<string, unknown> | null): void {
  act(() => {
    if (over === null) {
      useFirstRunStore.setState({ entries: {} })
      return
    }
    const doc = parseFirstRunDoc({
      enabled: true,
      hidden: false,
      finished: false,
      rows: [{ id: 'tunnel', state: 'todo', opens: 'tunnel' }],
      ...over,
    })
    useFirstRunStore.setState({ entries: { [primaryScope().id]: { doc, error: null } } })
  })
}
const listOpen = (): void => setDoc({})
const listHidden = (): void => setDoc({ hidden: true })

function Sidebar(): React.JSX.Element {
  const width = useSidebarDisplayWidth()
  const ref = useRef<HTMLDivElement>(null)
  useSidebarFloorMotion(ref, width)
  return <div ref={ref} data-testid="sidebar" style={{ width }} />
}
const sidebar = (): HTMLElement => document.querySelector('[data-testid="sidebar"]') as HTMLElement

type Made = { el: Element; keyframes: Keyframe[]; opts: KeyframeAnimationOptions; cancel: ReturnType<typeof vi.fn> }
let made: Made[] = []

function mockReducedMotion(reduce: boolean): void {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    writable: true,
    value: (query: string) => ({
      matches: reduce && query.includes('prefers-reduced-motion'),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }),
  })
}

beforeEach(() => {
  resetFirstRunForTests()
  setSidebarResizing(false)
  useSidebarStore.setState({ isCollapsed: false, width: 200 })
  h.freeze.mockClear()
  h.release.mockClear()
  made = []
  mockReducedMotion(false)
  ;(HTMLElement.prototype as { animate?: unknown }).animate = function (
    this: HTMLElement,
    keyframes: Keyframe[],
    opts: KeyframeAnimationOptions,
  ) {
    const a = { finished: new Promise(() => {}), cancel: vi.fn() }
    made.push({ el: this, keyframes, opts, cancel: a.cancel })
    return a
  }
  document.documentElement.style.setProperty('--motion-duration-slow', '320ms')
  document.documentElement.style.setProperty('--motion-ease-standard', 'cubic-bezier(0.22, 1, 0.36, 1)')
})

afterEach(() => {
  cleanup()
  delete (HTMLElement.prototype as { animate?: unknown }).animate
  delete (window as { matchMedia?: unknown }).matchMedia
  document.documentElement.style.removeProperty('--motion-duration-slow')
  document.documentElement.style.removeProperty('--motion-ease-standard')
  setSidebarResizing(false)
})

describe('the sidebar floor motion', () => {
  it('widens to the floor as the list opens, with the fold, on the slow motion tokens', () => {
    listHidden()
    render(<Sidebar />)
    expect(sidebar().style.width).toBe('200px')
    listOpen()
    // The target is drawn at once; the animation runs from the old width.
    expect(sidebar().style.width).toBe(`${FLOOR}px`)
    expect(made).toHaveLength(1)
    expect(made[0].el).toBe(sidebar())
    expect(made[0].keyframes).toEqual([{ width: '200px' }, { width: `${FLOOR}px` }])
    expect(made[0].opts).toEqual({ duration: 320, easing: 'cubic-bezier(0.22, 1, 0.36, 1)' })
    // The Browser page is a picture while the sidebar moves.
    expect(h.freeze).toHaveBeenCalledTimes(1)
    // The saved width is untouched: the floor is a display minimum.
    expect(useSidebarStore.getState().width).toBe(200)
  })

  it('narrows back to the saved width as the list folds', () => {
    listOpen()
    render(<Sidebar />)
    listHidden()
    expect(made).toHaveLength(1)
    expect(made[0].keyframes).toEqual([{ width: `${FLOOR}px` }, { width: '200px' }])
  })

  it('narrows back when setup finishes with the list open', () => {
    listOpen()
    render(<Sidebar />)
    setDoc({ finished: true })
    expect(made).toHaveLength(1)
    expect(made[0].keyframes).toEqual([{ width: `${FLOOR}px` }, { width: '200px' }])
  })

  it('does nothing when the saved width is at or above the floor', () => {
    for (const w of [FLOOR, 320]) {
      useSidebarStore.setState({ width: w })
      listHidden()
      const r = render(<Sidebar />)
      listOpen()
      listHidden()
      r.unmount()
    }
    expect(made).toEqual([])
  })

  it('does nothing at boot with the list already open, or after a server switch', () => {
    setDoc(null)
    render(<Sidebar />)
    listOpen()
    expect(sidebar().style.width).toBe(`${FLOOR}px`)
    setDoc(null)
    listOpen()
    expect(made).toEqual([])
  })

  it('is instant under reduced motion', () => {
    mockReducedMotion(true)
    listHidden()
    render(<Sidebar />)
    listOpen()
    expect(sidebar().style.width).toBe(`${FLOOR}px`)
    listHidden()
    expect(sidebar().style.width).toBe('200px')
    expect(made).toEqual([])
  })

  it('never animates during a drag on the resize edge', () => {
    listHidden()
    render(<Sidebar />)
    setSidebarResizing(true)
    listOpen()
    act(() => useSidebarStore.getState().setWidth(300))
    act(() => useSidebarStore.getState().setWidth(240))
    listHidden()
    expect(made).toEqual([])
  })

  it('a drag stops a motion under way', () => {
    listHidden()
    render(<Sidebar />)
    listOpen()
    expect(made).toHaveLength(1)
    act(() => useSidebarStore.getState().setWidth(300))
    expect(made[0].cancel).toHaveBeenCalled()
    expect(h.release).toHaveBeenCalled()
    expect(made).toHaveLength(1)
  })
})
