// @vitest-environment jsdom
//
// While the setup checklist shows, the sidebar isn't drawn narrower than
// SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH and its collapse button is hidden. A
// narrower saved width is drawn at the floor and kept, so it comes back once
// the checklist goes.

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
  useFirstRunStore,
  useSetupChecklistShowing,
} from '@/stores/first-run'
import { nextSavedSidebarWidth, useSidebarDisplayWidth, useSidebarStore } from '@/stores/sidebar'
import ResizeHandle from '@/components/Sidebar/ResizeHandle'
import { SidebarCollapseButton } from '@/components/Sidebar/SidebarCollapseButton'

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
})
