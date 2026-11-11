// @vitest-environment jsdom
//
// While the setup checklist shows, the sidebar isn't drawn narrower than
// SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH and its collapse button is hidden. A
// narrower saved width is drawn at the floor and kept, so it comes back once
// the checklist goes. ⌘B (the store's toggle) doesn't collapse the nav either.

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
  parseFirstRunDoc,
  resetFirstRunForTests,
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

afterEach(() => cleanup())

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

describe("the hidden list's Get started bubble", () => {
  it('counts as dismissed: no floor, the collapse button is back and ⌘B collapses', () => {
    useSidebarStore.setState({ width: 200 })
    const { result } = renderHook(() => useSidebarDisplayWidth())
    showChecklist()
    expect(result.current).toBe(SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH)
    hideChecklist()
    expect(showsCollapsed(useFirstRunStore.getState().entries[primaryScope().id]?.doc)).toBe(true)
    expect(isSetupChecklistShowing()).toBe(false)
    expect(result.current).toBe(200)
    render(<SidebarCollapseButton />)
    expect(screen.queryByRole('button', { name: 'Collapse workspaces sidebar' })).not.toBeNull()
    useSidebarStore.getState().toggle()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
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

  it('is hidden while the checklist shows and back after', () => {
    render(<SidebarCollapseButton />)
    expect(collapse()).not.toBeNull()
    showChecklist()
    expect(collapse()).toBeNull()
    hideChecklist()
    expect(collapse()).not.toBeNull()
  })

  it("leaves an already collapsed nav's open button on the rail", () => {
    useSidebarStore.setState({ isCollapsed: true })
    showChecklist()
    render(<SidebarCollapseButton />)
    expect(screen.queryByRole('button', { name: 'Open workspaces sidebar' })).not.toBeNull()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
  })

  it('in the nav footer it keeps its slot while hidden: invisible, out of the tab order and the tree, and inert', () => {
    render(<SidebarCollapseButton reserveSlot />)
    const slot = (): HTMLElement => document.querySelector('[data-nav-collapse]') as HTMLElement
    // The footer's button: the shared quiet Button (cut corners in Terminal).
    expect(slot().getAttribute('data-k2-chamfer')).toBe('control')
    expect(slot().className).toContain('border-[var(--color-control-edge)]')
    // Not Frost: its rim is under 3:1 on the nav, the control edge isn't.
    expect(slot().hasAttribute('data-material')).toBe(false)
    expect(collapse()).toBe(slot())
    showChecklist()
    // Still drawn (so Add Workspace keeps its width), but not there for anyone.
    expect(slot()).not.toBeNull()
    expect(slot().className).toContain('invisible')
    expect(slot().getAttribute('aria-hidden')).toBe('true')
    expect(slot().tabIndex).toBe(-1)
    expect(collapse()).toBeNull()
    fireEvent.click(slot())
    expect(useSidebarStore.getState().isCollapsed).toBe(false)
    hideChecklist()
    expect(slot().className).not.toContain('invisible')
    expect(slot().getAttribute('aria-hidden')).toBeNull()
    expect(collapse()).toBe(slot())
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

  it("doesn't collapse an open nav while the checklist shows", () => {
    showChecklist()
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
    useSidebarStore.getState().collapse()
    expect(collapsed()).toBe(false)
  })

  it('still opens a collapsed nav while the checklist shows', () => {
    useSidebarStore.setState({ isCollapsed: true })
    showChecklist()
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
  })

  it('collapses as before once the checklist stops showing', () => {
    showChecklist()
    hideChecklist()
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(true)
    useSidebarStore.getState().toggle()
    expect(collapsed()).toBe(false)
    useSidebarStore.getState().collapse()
    expect(collapsed()).toBe(true)
  })
})
