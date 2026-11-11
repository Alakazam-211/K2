// @vitest-environment jsdom
//
// While the setup list is open, the sidebar isn't drawn narrower than
// SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH. A narrower saved width is drawn at the
// floor and kept, so it comes back once the list goes. Until setup is
// finished (open or hidden as Get started), the collapse button is gone, the
// footer's main action takes the full row, and ⌘B (the store's toggle)
// doesn't collapse the nav; when setup finishes « slides back in.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => new Promise(() => {})),
  settingsUpdate: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(() => new Promise(() => {})),
  daemonCliPost: vi.fn(() => new Promise(() => {})),
}))

import { act } from 'react'
import { cleanup, fireEvent, render, renderHook, screen } from '@testing-library/react'
import {
  SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
} from '../../shared/constants'
import { primaryScope } from '@/kessel/server-scope'
import {
  isSetupChecklistShowing,
  isSetupNotFinished,
  parseFirstRunDoc,
  resetFirstRunForTests,
  setupNotFinished,
  showsCollapsed,
  useFirstRunStore,
  useSetupChecklistShowing,
} from '@/stores/first-run'
import { nextSavedSidebarWidth, useSidebarDisplayWidth, useSidebarStore } from '@/stores/sidebar'
import ResizeHandle from '@/components/Sidebar/ResizeHandle'
import { SidebarCollapseButton } from '@/components/Sidebar/SidebarCollapseButton'
import { NAV_FOOTER_ROW_CLASS, NavFooterButton, NavFooterPlus } from '@/components/Sidebar/NavFooterButton'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const ROWS = [
  { id: 'llm_sign_in', state: 'todo', opens: 'llm_sign_in' },
  { id: 'tunnel', state: 'todo', opens: 'tunnel' },
]

function setDoc(over: Record<string, unknown>): void {
  const doc = parseFirstRunDoc({ enabled: true, hidden: false, finished: false, rows: ROWS, ...over })
  act(() => {
    useFirstRunStore.setState({ entries: { [primaryScope().id]: { doc, error: null } } })
  })
}

const showChecklist = (): void => setDoc({})
const hideChecklist = (): void => setDoc({ hidden: true })
const finishSetup = (): void => setDoc({ finished: true })

/** `prefers-reduced-motion` for `useReducedMotion` (jsdom has no matchMedia). */
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

function drag(dx: number): void {
  const edge = document.querySelector('[data-resize-edge]') as HTMLElement
  fireEvent.mouseDown(edge, { clientX: 500 })
  fireEvent.mouseMove(document, { clientX: 500 + dx })
  fireEvent.mouseUp(document)
}

beforeEach(() => {
  resetFirstRunForTests()
  useSidebarStore.setState({ isCollapsed: false, width: SIDEBAR_DEFAULT_WIDTH })
})

afterEach(() => {
  cleanup()
  delete (window as { matchMedia?: unknown }).matchMedia
})

describe('setup checklist showing', () => {
  it('is true only while the rows render', () => {
    expect(isSetupChecklistShowing()).toBe(false)
    const { result } = renderHook(() => useSetupChecklistShowing())
    expect(result.current).toBe(false)
    showChecklist()
    expect(isSetupChecklistShowing()).toBe(true)
    expect(result.current).toBe(true)
    for (const off of [{ enabled: false }, { hidden: true }, { finished: true }, { rows: [] }]) {
      setDoc(off)
      expect(isSetupChecklistShowing()).toBe(false)
      expect(result.current).toBe(false)
    }
  })
})

describe("the hidden list's Get started button", () => {
  it('drops the floor but not the collapse lock: no «, ⌘B blocked, until setup is finished', () => {
    useSidebarStore.setState({ width: 200 })
    const { result } = renderHook(() => useSidebarDisplayWidth())
    showChecklist()
    expect(result.current).toBe(SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH)
    hideChecklist()
    expect(showsCollapsed(useFirstRunStore.getState().entries[primaryScope().id]?.doc)).toBe(true)
    // The floor is the open list's only.
    expect(isSetupChecklistShowing()).toBe(false)
    expect(result.current).toBe(200)
    // Hiding isn't finishing: « stays gone and ⌘B stays blocked.
    expect(isSetupNotFinished()).toBe(true)
    render(<SidebarCollapseButton />)
    expect(screen.queryByRole('button', { name: 'Collapse workspaces sidebar' })).toBeNull()
    useSidebarStore.getState().toggle()
    expect(useSidebarStore.getState().isCollapsed).toBe(false)
    useSidebarStore.getState().collapse()
    expect(useSidebarStore.getState().isCollapsed).toBe(false)
    // Finished: both come back.
    finishSetup()
    expect(isSetupNotFinished()).toBe(false)
    expect(screen.queryByRole('button', { name: 'Collapse workspaces sidebar' })).not.toBeNull()
    act(() => useSidebarStore.getState().toggle())
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
  })
})

describe('setup not finished', () => {
  it('is the switch on and the list not finished, open or hidden', () => {
    const doc = (over: Record<string, unknown>) =>
      parseFirstRunDoc({ enabled: true, hidden: false, finished: false, rows: ROWS, ...over })
    expect(setupNotFinished(null)).toBe(false)
    expect(setupNotFinished(doc({}))).toBe(true)
    expect(setupNotFinished(doc({ hidden: true }))).toBe(true)
    expect(setupNotFinished(doc({ finished: true }))).toBe(false)
    expect(setupNotFinished(doc({ hidden: true, finished: true }))).toBe(false)
    expect(setupNotFinished(doc({ enabled: false }))).toBe(false)
  })
})

describe('sidebar width clamp', () => {
  it('uses the checklist floor while it shows and the normal range otherwise', () => {
    const s = useSidebarStore.getState()
    s.setWidth(100)
    expect(useSidebarStore.getState().width).toBe(SIDEBAR_MIN_WIDTH)
    s.setWidth(999)
    expect(useSidebarStore.getState().width).toBe(SIDEBAR_MAX_WIDTH)

    s.setWidth(300)
    showChecklist()
    s.setWidth(200)
    expect(useSidebarStore.getState().width).toBe(SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH)
    s.setWidth(999)
    expect(useSidebarStore.getState().width).toBe(SIDEBAR_MAX_WIDTH)

    hideChecklist()
    s.setWidth(200)
    expect(useSidebarStore.getState().width).toBe(200)
  })

  it('draws a narrower saved width at the floor while the checklist shows, then the saved width again', () => {
    useSidebarStore.setState({ width: 200 })
    const { result } = renderHook(() => useSidebarDisplayWidth())
    expect(result.current).toBe(200)
    showChecklist()
    expect(result.current).toBe(SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH)
    expect(useSidebarStore.getState().width).toBe(200)
    hideChecklist()
    expect(result.current).toBe(200)
  })

  it('leaves a sidebar wider than the floor alone', () => {
    useSidebarStore.setState({ width: 320 })
    const { result } = renderHook(() => useSidebarDisplayWidth())
    showChecklist()
    expect(result.current).toBe(320)
  })

  it('a drag while the checklist shows never lowers a narrower saved width', () => {
    useSidebarStore.setState({ width: 200 })
    showChecklist()
    render(<ResizeHandle />)

    drag(-80)
    expect(useSidebarStore.getState().width).toBe(200)
    drag(0)
    expect(useSidebarStore.getState().width).toBe(200)

    // Wider than the floor is the person's new width (from the drawn 260).
    drag(30)
    expect(useSidebarStore.getState().width).toBe(SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH + 30)
    drag(-200)
    expect(useSidebarStore.getState().width).toBe(SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH)

    hideChecklist()
    drag(-200)
    expect(useSidebarStore.getState().width).toBe(SIDEBAR_MIN_WIDTH)
  })

  it('nextSavedSidebarWidth keeps a narrower saved width at the floor', () => {
    expect(nextSavedSidebarWidth(200, 150, true)).toBe(200)
    expect(nextSavedSidebarWidth(200, 270, true)).toBe(270)
    expect(nextSavedSidebarWidth(300, 150, true)).toBe(SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH)
    expect(nextSavedSidebarWidth(300, 150, false)).toBe(SIDEBAR_MIN_WIDTH)
  })
})

describe('sidebar collapse button', () => {
  const collapse = (): HTMLElement | null => screen.queryByRole('button', { name: 'Collapse workspaces sidebar' })

  it('is hidden until setup is finished, with the list open or hidden, and back after', () => {
    render(<SidebarCollapseButton />)
    expect(collapse()).not.toBeNull()
    showChecklist()
    expect(collapse()).toBeNull()
    hideChecklist()
    expect(collapse()).toBeNull()
    showChecklist()
    expect(collapse()).toBeNull()
    finishSetup()
    expect(collapse()).not.toBeNull()
  })

  it("leaves an already collapsed nav collapsed, with the rail's open button", () => {
    useSidebarStore.setState({ isCollapsed: true })
    showChecklist()
    render(<SidebarCollapseButton />)
    expect(screen.queryByRole('button', { name: 'Open workspaces sidebar' })).not.toBeNull()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
    hideChecklist()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
    expect(screen.queryByRole('button', { name: 'Open workspaces sidebar' })).not.toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Open workspaces sidebar' }))
    expect(useSidebarStore.getState().isCollapsed).toBe(false)
  })

  describe('in the nav footer', () => {
    const slot = (): HTMLElement => document.querySelector('[data-nav-collapse-slot]') as HTMLElement
    const btn = (): HTMLElement => document.querySelector('[data-nav-collapse]') as HTMLElement
    let widthSpy: { mockRestore: () => void } | null = null
    beforeEach(() => {
      // jsdom lays nothing out: give « its Terminal width.
      widthSpy = vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (this: HTMLElement) {
        return this.hasAttribute('data-nav-collapse') ? 32 : 0
      })
    })
    afterEach(() => widthSpy?.mockRestore())

    function footer(): void {
      render(
        <div className={NAV_FOOTER_ROW_CLASS}>
          <NavFooterButton label="Add Workspace" icon={<NavFooterPlus />} />
          <SidebarCollapseButton footer />
        </div>,
      )
    }

    it('is the shared quiet Button, not Frost', () => {
      footer()
      expect(btn().getAttribute('data-k2-chamfer')).toBe('control')
      expect(btn().className).toContain('border-[var(--color-control-edge)]')
      expect(btn().hasAttribute('data-material')).toBe(false)
      expect(collapse()).toBe(btn())
    })

    it('closes to nothing while setup is not finished, so the add button takes the full row', () => {
      footer()
      // Open: « and the row's gap (8px) beside the add button.
      expect(slot().getAttribute('data-nav-collapse-slot')).toBe('open')
      expect(slot().style.width).toBe('40px')
      // The row has no gap of its own; the slot carries it.
      expect(NAV_FOOTER_ROW_CLASS).not.toMatch(/\bgap-/)
      expect(screen.getByRole('button', { name: 'Add Workspace' }).className).toContain('flex-1')
      for (const step of [showChecklist, hideChecklist]) {
        step()
        expect(slot().getAttribute('data-nav-collapse-slot')).toBe('closed')
        expect(slot().style.width).toBe('0px')
        expect(slot().className).toContain('overflow-hidden')
        // Closing is instant (a list loading in never animates « away).
        expect(slot().style.transition).toBe('none')
        // Not there for anyone, and inert.
        expect(btn().getAttribute('aria-hidden')).toBe('true')
        expect(btn().tabIndex).toBe(-1)
        expect(btn().hasAttribute('inert')).toBe(true)
        expect(collapse()).toBeNull()
        fireEvent.click(btn())
        expect(useSidebarStore.getState().isCollapsed).toBe(false)
      }
    })

    it('slides « back in when setup finishes: the add button shrinks as the slot opens', () => {
      mockReducedMotion(false)
      vi.useFakeTimers()
      try {
        showChecklist()
        footer()
        expect(slot().style.width).toBe('0px')
        finishSetup()
        expect(slot().getAttribute('data-nav-collapse-slot')).toBe('open')
        expect(slot().style.width).toBe('40px')
        expect(slot().style.transition).toBe('width var(--motion-duration-slow) var(--motion-ease-standard)')
        // Clipped while it slides, then it lets go (so the focus ring shows).
        expect(slot().className).toContain('overflow-hidden')
        act(() => {
          vi.advanceTimersByTime(400)
        })
        expect(slot().className).not.toContain('overflow-hidden')
        expect(btn().getAttribute('aria-hidden')).toBeNull()
        expect(btn().hasAttribute('inert')).toBe(false)
        expect(collapse()).toBe(btn())
        fireEvent.click(btn())
        expect(useSidebarStore.getState().isCollapsed).toBe(true)
      } finally {
        vi.useRealTimers()
      }
    })

    it('under reduced motion « comes back at once', () => {
      mockReducedMotion(true)
      showChecklist()
      footer()
      finishSetup()
      expect(slot().style.width).toBe('40px')
      expect(slot().style.transition).toBe('none')
    })
  })
})

describe('nav footer action', () => {
  it('is the shared quiet Button with a one-line label that drops to the icon when the row is narrow', () => {
    render(
      <div className={NAV_FOOTER_ROW_CLASS}>
        <NavFooterButton label="Add Workspace" icon={<NavFooterPlus />} />
      </div>,
    )
    const btn = screen.getByRole('button', { name: 'Add Workspace' })
    expect(btn.getAttribute('data-k2-chamfer')).toBe('control')
    expect(btn.className).toContain('border-[var(--color-control-edge)]')
    expect(btn.hasAttribute('data-material')).toBe(false)
    expect(btn.getAttribute('data-frost-shape')).toBe('pill')
    expect(btn.className).toContain('whitespace-nowrap')
    expect(btn.getAttribute('title')).toBe('Add Workspace')
    expect(btn.parentElement!.className).toContain('@container')
    const label = btn.querySelector('[data-nav-footer-label]')!
    expect(label.className).toContain('truncate')
    // Below 12.5rem the label is for screen readers only, so the name stays.
    expect(label.className).toContain('@max-[12.5rem]:sr-only')
  })
})

describe('sidebar toggle (⌘B)', () => {
  const collapsed = (): boolean => useSidebarStore.getState().isCollapsed

  it("doesn't collapse an open nav while the list shows", () => {
    showChecklist()
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
    useSidebarStore.getState().collapse()
    expect(collapsed()).toBe(false)
  })

  it("doesn't collapse an open nav while the list is hidden but setup isn't finished", () => {
    showChecklist()
    hideChecklist()
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
    useSidebarStore.getState().collapse()
    expect(collapsed()).toBe(false)
  })

  it('still opens a collapsed nav before setup is finished', () => {
    useSidebarStore.setState({ isCollapsed: true })
    hideChecklist()
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
  })

  it('collapses as before once setup is finished', () => {
    showChecklist()
    finishSetup()
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(true)
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
    useSidebarStore.getState().collapse()
    expect(collapsed()).toBe(true)
  })

  it('collapses with the switch off', () => {
    setDoc({ enabled: false })
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(true)
  })
})
